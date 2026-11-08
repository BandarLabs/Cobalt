//! The article's body, rewritten as the small XHTML a book holds.
//!
//! Reader-mode extraction finds the article and throws away most of the page
//! around it, but what it hands back is still the page's own markup: classes,
//! styles, wrappers nested twelve deep, lazy pictures that name their real
//! source in a data attribute, and the odd share bar or "Advertisement" label
//! that sat inside the story itself. A book page must be well-formed XML and
//! should hold nothing a reader would not want to see.
//!
//! So the extracted HTML is walked once and rebuilt from a short list of
//! elements a book needs (headings, paragraphs, lists, quotes, code, tables,
//! figures and their captions, and the inline emphasis inside them). Anything
//! else is either unwrapped, keeping its text, or left out with its text. The
//! rebuilt tree is plain data, so the pictures can be fetched afterwards and
//! the ones that failed taken out before anything is written: a picture that
//! could not be fetched leaves no broken reference and no orphaned caption.

use std::fmt::Write as _;

use dom_query::NodeRef;

use crate::address;

/// Elements whose content is never part of the article.
const DROPPED: [&str; 31] = [
    "script", "style", "noscript", "template", "iframe", "object", "embed", "video", "audio",
    "canvas", "svg", "form", "input", "button", "select", "textarea", "nav", "aside", "footer",
    "dialog", "menu", "map", "area", "link", "meta", "head", "title", "source", "track", "label",
    "fieldset",
];

/// Words that, as a whole word of an element's class or id, only ever name
/// furniture: the share bars, sign-up boxes, advertisements and lists of
/// other stories that a page sets inside the article as well as around it.
///
/// Matched as whole words, so `ad` catches `ad-slot` but not `header` or
/// `download`. Not applied inside code, where a highlighter's `comment`
/// class is part of the listing.
const FURNITURE_WORDS: [&str; 31] = [
    "ad",
    "ads",
    "advert",
    "advertisement",
    "adslot",
    "dfp",
    "sponsor",
    "sponsored",
    "promo",
    "share",
    "sharing",
    "sharebar",
    "social",
    "newsletter",
    "subscribe",
    "subscription",
    "signup",
    "paywall",
    "related",
    "recommended",
    "recirculation",
    "outbrain",
    "taboola",
    "consent",
    "gdpr",
    "comments",
    "comment",
    "popup",
    "modal",
    "editsection",
    "noprint",
];

/// Class and id spellings of a cookie notice. Matched as text rather than as
/// words, because a recipe is entitled to a class called `cookie-recipe`.
const COOKIE_NOTICES: [&str; 5] = [
    "cookie-banner",
    "cookie-notice",
    "cookie-consent",
    "cookiebanner",
    "cookie_notice",
];

/// Whole lines a page puts between paragraphs that say nothing about the
/// story. Compared lowercased, without trailing punctuation.
const FURNITURE_LINES: [&str; 16] = [
    "advertisement",
    "advertisements",
    "ad",
    "sponsored",
    "sponsored content",
    "story continues below advertisement",
    "article continues below advertisement",
    "article continues below",
    "continue reading below",
    "skip advertisement",
    "scroll to continue with content",
    "share this article",
    "share this story",
    "share",
    "listen to this article",
    "copy link",
];

/// How a line pointing at another story starts. Such a line is left out only
/// when it is mostly link, so a sentence that starts "Related: the tide" and
/// goes on in prose is kept.
const ELSEWHERE_PREFIXES: [&str; 9] = [
    "read more",
    "read also",
    "also read",
    "related",
    "see also:",
    "recommended",
    "more:",
    "don't miss",
    "most read",
];

/// Attributes a lazy page writes its real picture into.
const LAZY_SOURCES: [&str; 9] = [
    "data-src",
    "data-lazy-src",
    "data-original",
    "data-original-src",
    "data-hi-res-src",
    "data-full-src",
    "data-lazy",
    "data-url",
    "data-image",
];

/// Attributes a page writes a list of picture sizes into.
const SOURCE_SETS: [&str; 3] = ["srcset", "data-srcset", "data-lazy-srcset"];

/// The picture width worth asking for: the reference panel's.
const WANTED_WIDTH: u32 = 1072;

/// Pictures declared this small are icons, avatars or spacers.
const ICON_SIDE: u32 = 48;

/// How deep the walk follows nested elements. A real article is a dozen
/// deep; past this, the text is kept and its structure is not.
const MAX_DEPTH: usize = 160;

/// One node of the rebuilt body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    Element(Element),
    Text(String),
    /// The picture at this index of [`Body::pictures`].
    Picture(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Element {
    pub name: &'static str,
    pub attributes: Vec<(&'static str, String)>,
    pub children: Vec<Node>,
}

impl Element {
    fn new(name: &'static str, children: Vec<Node>) -> Self {
        Self {
            name,
            attributes: Vec::new(),
            children,
        }
    }
}

/// A picture the body refers to, before it has been fetched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Picture {
    pub address: String,
    pub alt: String,
}

/// The rebuilt body.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Body {
    pub nodes: Vec<Node>,
    pub pictures: Vec<Picture>,
}

struct Walk<'a> {
    page: &'a str,
    title: String,
    /// Letters in the whole article, so that a wrapper whose class happens
    /// to name furniture is not mistaken for some when it holds the story.
    letters: usize,
    pictures: Vec<Picture>,
    /// Inside `pre` or `code`, where whitespace is the content and class
    /// names belong to a highlighter.
    code: usize,
}

/// Rebuilds the article at `root` for a book.
///
/// `page` is the address the page came from, which relative sources are
/// resolved against. `title` and `byline` are what the title page will say,
/// so that a heading or a credit line that only repeats them is not printed
/// twice.
#[must_use]
pub fn rebuild(root: &NodeRef<'_>, page: &str, title: &str, byline: Option<&str>) -> Body {
    let mut walk = Walk {
        page,
        title: comparable(title),
        letters: letters(root),
        pictures: Vec::new(),
        code: 0,
    };
    let nodes = walk.flow(&root.children(), 0);
    let mut body = Body {
        nodes,
        pictures: walk.pictures,
    };
    tidy(&mut body.nodes);
    drop_header_lines(&mut body.nodes, byline);
    body
}

/// The text of nodes, for judging a line.
fn plain_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Element(element) => plain_text(&element.children, out),
            Node::Picture(_) => {}
        }
    }
}

/// Whether nodes are nothing but links, and the whitespace between them.
fn all_link(nodes: &[Node]) -> bool {
    let mut any = false;
    for node in nodes {
        match node {
            Node::Text(text) if text.trim().is_empty() => {}
            Node::Element(element) if element.name == "a" => any = true,
            Node::Element(element) if matches!(element.name, "strong" | "em") => {
                if !all_link(&element.children) {
                    return false;
                }
                any = true;
            }
            _ => return false,
        }
    }
    any
}

const MONTH_WORDS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Whether a short line is a date: a year, and a month by name or number.
fn is_date_line(text: &str) -> bool {
    let lower = text.to_lowercase();
    let has_year = lower
        .split(|c: char| !c.is_ascii_digit())
        .any(|digits| digits.len() == 4 && (digits.starts_with("19") || digits.starts_with("20")));
    let has_month = lower
        .split(|c: char| !c.is_alphabetic())
        .any(|word| word.len() >= 3 && MONTH_WORDS.iter().any(|month| word.starts_with(month)))
        || lower.as_bytes().windows(3).any(|window| {
            window[0].is_ascii_digit()
                && matches!(window[1], b'-' | b'/' | b'.')
                && window[2].is_ascii_digit()
        });
    has_year && has_month && text.split_whitespace().count() <= 8
}

/// Leaves out the article's own header lines that the title page already
/// says: a credit naming the author, a dateline, and a section label above
/// the headline that is only a link. Only the lines before the story starts
/// are looked at, so the same words later in the article are kept.
fn drop_header_lines(nodes: &mut Vec<Node>, byline: Option<&str>) {
    let byline = byline.map(comparable).filter(|byline| !byline.is_empty());
    let mut index = 0;
    while index < nodes.len() {
        let Node::Element(element) = &nodes[index] else {
            index += 1;
            continue;
        };
        // A lead photograph may sit above the header lines.
        if element.name == "figure" {
            index += 1;
            continue;
        }
        if element.name != "p" {
            break;
        }
        let mut text = String::new();
        plain_text(&element.children, &mut text);
        let key = comparable(&text);
        // The story has started.
        if key.len() > 120 {
            break;
        }
        let credit = byline
            .as_deref()
            .is_some_and(|byline| key.contains(byline) && key.len() <= byline.len() + 24);
        let label = key.len() <= 40 && all_link(&element.children);
        if credit || label || (key.len() <= 60 && is_date_line(&key)) {
            nodes.remove(index);
        } else {
            index += 1;
        }
    }
}

fn is_heading(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// Elements that start a block of their own.
fn is_block(name: &str) -> bool {
    is_heading(name)
        || matches!(
            name,
            "p" | "ul"
                | "ol"
                | "li"
                | "dl"
                | "dt"
                | "dd"
                | "blockquote"
                | "pre"
                | "figure"
                | "figcaption"
                | "table"
                | "caption"
                | "thead"
                | "tbody"
                | "tfoot"
                | "tr"
                | "th"
                | "td"
                | "hr"
                | "div"
                | "section"
                | "article"
                | "main"
                | "header"
                | "details"
                | "summary"
                | "center"
                | "address"
                | "hgroup"
        )
}

/// Lowercased words of text with whitespace collapsed, for comparing.
fn comparable(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(character);
            space = false;
        }
    }
    out
}

fn words_of(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Whether the element's class or id names furniture.
pub(crate) fn names_furniture(node: &NodeRef<'_>) -> bool {
    let named = |value: Option<&str>| {
        value.is_some_and(|value| {
            let lower = value.to_ascii_lowercase();
            words_of(value).any(|word| FURNITURE_WORDS.contains(&word.as_str()))
                || COOKIE_NOTICES.iter().any(|notice| lower.contains(notice))
        })
    };
    named(node.attr("class").as_deref()) || named(node.attr("id").as_deref())
}

/// Letters in an element's text, ignoring whitespace.
fn letters(node: &NodeRef<'_>) -> usize {
    node.text().chars().filter(|c| !c.is_whitespace()).count()
}

/// The text of a line with trailing punctuation removed, for matching
/// against the furniture lines.
fn line_key(text: &str) -> String {
    comparable(text)
        .trim_end_matches(|c: char| c.is_ascii_punctuation() || c == '…')
        .to_owned()
}

/// Whether a block is a line pointing at other stories, or a label.
fn is_furniture_line(node: &NodeRef<'_>) -> bool {
    let text = node.text();
    // Labels and pointers are a line or two; anything longer is prose.
    if text.len() > 400 {
        return false;
    }
    let key = line_key(&text);
    if key.is_empty() {
        return false;
    }
    if FURNITURE_LINES.contains(&key.as_str()) {
        return true;
    }
    if ELSEWHERE_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
    {
        let linked: usize = node
            .descendants()
            .iter()
            .filter(|child| child.node_name().as_deref() == Some("a"))
            .map(|link| link.text().chars().count())
            .sum();
        let total = text.chars().filter(|c| !c.is_whitespace()).count().max(1);
        let linked_letters = linked.min(total);
        return linked_letters * 2 >= total;
    }
    false
}

fn number(value: Option<&str>) -> Option<u32> {
    let value = value?;
    let digits: String = value
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Splits a `srcset` into addresses and the width each stands for.
///
/// A width descriptor (`800w`) is taken at its word; a density descriptor
/// (`2x`) is turned into a width using the width the page drew the picture
/// at, or a typical column width when it did not say.
fn source_set(value: &str, drawn_width: Option<u32>) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        // An address runs to whitespace; a comma right after it ends the
        // candidate. Commas inside an address are part of it.
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let mut address = &rest[..end];
        rest = rest[end..].trim_start();
        let mut descriptor = "";
        if let Some(stripped) = address.strip_suffix(',') {
            address = stripped;
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            descriptor = rest[..end].trim();
            rest = &rest[end..];
        }
        rest = rest.trim_start_matches(',').trim_start();
        if address.is_empty() {
            continue;
        }
        let base = drawn_width.unwrap_or(600);
        let width = if let Some(width) = descriptor.strip_suffix('w') {
            width.trim().parse().unwrap_or(base)
        } else if let Some(density) = descriptor.strip_suffix('x') {
            let density: f32 = density.trim().parse().unwrap_or(1.0);
            // A density is a small positive number; anything else is noise.
            let density = density.clamp(0.25, 4.0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "a clamped density times a picture width is a small positive number"
            )]
            let width = (base as f32 * density) as u32;
            width
        } else {
            base
        };
        out.push((address.to_owned(), width));
    }
    out
}

/// Whether an address names a format the pipeline cannot decode.
fn unreadable_format(address: &str) -> bool {
    let path = address.split(['?', '#']).next().unwrap_or(address);
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["svg", "svgz", "avif"]
                .iter()
                .any(|unreadable| extension.eq_ignore_ascii_case(unreadable))
        })
}

/// Whether an address is a known tracker rather than a picture.
fn is_tracker(address: &str) -> bool {
    const TRACKERS: [&str; 10] = [
        "/pixel",
        "pixel.",
        "/beacon",
        "/tracking",
        "/track/",
        "1x1.",
        "spacer.gif",
        "blank.gif",
        "facebook.com/tr",
        "doubleclick.net",
    ];
    let lower = address.to_ascii_lowercase();
    TRACKERS.iter().any(|tracker| lower.contains(tracker))
}

impl Walk<'_> {
    /// Block context: inline runs are gathered into paragraphs.
    fn flow(&mut self, children: &[NodeRef<'_>], depth: usize) -> Vec<Node> {
        self.blocks(children, depth, true)
    }

    /// Block context that leaves inline runs as they are, for list items,
    /// table cells and captions, which hold either.
    fn mixed(&mut self, children: &[NodeRef<'_>], depth: usize) -> Vec<Node> {
        self.blocks(children, depth, false)
    }

    fn blocks(&mut self, children: &[NodeRef<'_>], depth: usize, wrap: bool) -> Vec<Node> {
        let mut out = Vec::new();
        let mut run: Vec<Node> = Vec::new();
        for child in children {
            if child.is_text() {
                run.push(self.text(child));
                continue;
            }
            if !child.is_element() {
                continue;
            }
            let name = child
                .node_name()
                .map(|name| name.to_string())
                .unwrap_or_default();
            if self.dropped(child, &name) {
                continue;
            }
            if is_block(&name) {
                flush(&mut run, &mut out, wrap);
                self.block(child, &name, depth + 1, &mut out);
            } else {
                run.extend(self.inline(child, &name, depth + 1));
            }
        }
        flush(&mut run, &mut out, wrap);
        out
    }

    fn dropped(&self, node: &NodeRef<'_>, name: &str) -> bool {
        if DROPPED.contains(&name) {
            return true;
        }
        if node.has_attr("hidden") || node.attr("aria-hidden").as_deref() == Some("true") {
            return true;
        }
        self.code == 0 && names_furniture(node) && letters(node) * 2 <= self.letters
    }

    fn text(&self, node: &NodeRef<'_>) -> Node {
        let text = node.text();
        if self.code > 0 {
            Node::Text(text.to_string())
        } else {
            Node::Text(collapse_whitespace(&text))
        }
    }

    fn block(&mut self, node: &NodeRef<'_>, name: &str, depth: usize, out: &mut Vec<Node>) {
        if depth > MAX_DEPTH {
            out.push(Node::Element(Element::new(
                "p",
                vec![Node::Text(collapse_whitespace(&node.text()))],
            )));
            return;
        }
        let children = node.children();
        match name {
            "p" | "summary" | "address" => {
                if is_furniture_line(node) {
                    return;
                }
                let inline = self.phrasing(&children, depth);
                push_paragraph(inline, out);
            }
            heading if is_heading(heading) => {
                if comparable(&node.text()) == self.title || is_furniture_line(node) {
                    return;
                }
                // The title page holds the one first-level heading.
                let name = match heading {
                    "h1" | "h2" => "h2",
                    "h3" => "h3",
                    "h4" => "h4",
                    "h5" => "h5",
                    _ => "h6",
                };
                out.push(Node::Element(Element::new(
                    name,
                    self.phrasing(&children, depth),
                )));
            }
            "ul" | "ol" => {
                if !is_furniture_line(node) {
                    out.push(self.list(node, name == "ol", depth));
                }
            }
            "li" => {
                // An item outside a list: keep it as a list of one.
                let content = self.mixed(&children, depth);
                out.push(Node::Element(Element::new(
                    "ul",
                    vec![Node::Element(Element::new("li", content))],
                )));
            }
            "dl" => out.push(Node::Element(self.definitions(node, depth))),
            "dt" | "dd" => {
                let inline = self.phrasing(&children, depth);
                push_paragraph(inline, out);
            }
            "blockquote" => {
                let content = self.flow(&children, depth);
                out.push(Node::Element(Element::new("blockquote", content)));
            }
            "pre" => {
                self.code += 1;
                let text = preformatted(node);
                self.code -= 1;
                if !text.trim().is_empty() {
                    out.push(Node::Element(Element::new("pre", vec![Node::Text(text)])));
                }
            }
            "figure" => out.push(self.figure(node, depth)),
            "figcaption" | "caption" => {
                // Met outside a figure or table: an ordinary line.
                let inline = self.phrasing(&children, depth);
                push_paragraph(inline, out);
            }
            "table" => self.table(node, depth, out),
            "thead" | "tbody" | "tfoot" | "tr" | "th" | "td" => {
                // Table parts outside a table: keep their text.
                out.extend(self.flow(&children, depth));
            }
            "hr" => out.push(Node::Element(Element::new("hr", Vec::new()))),
            _ => {
                // div, section, article and the other wrappers: their content
                // without them, unless the whole wrapper is a furniture line.
                if is_furniture_line(node) {
                    return;
                }
                out.extend(self.flow(&children, depth));
            }
        }
    }

    fn list(&mut self, node: &NodeRef<'_>, ordered: bool, depth: usize) -> Node {
        let mut items = Vec::new();
        for child in node.children() {
            let child_name = child
                .node_name()
                .map(|name| name.to_string())
                .unwrap_or_default();
            if child.is_element() && self.dropped(&child, &child_name) {
                continue;
            }
            let content = if child_name == "li" {
                self.mixed(&child.children(), depth + 1)
            } else if child.is_element() || child.is_text() {
                self.mixed(std::slice::from_ref(&child), depth + 1)
            } else {
                continue;
            };
            if has_content(&content) {
                items.push(Node::Element(Element::new("li", content)));
            }
        }
        let mut list = Element::new(if ordered { "ol" } else { "ul" }, items);
        if ordered {
            if let Some(start) = number(node.attr("start").as_deref()) {
                list.attributes.push(("start", start.to_string()));
            }
        }
        Node::Element(list)
    }

    fn definitions(&mut self, node: &NodeRef<'_>, depth: usize) -> Element {
        let mut terms = Vec::new();
        for child in node.children() {
            match child.node_name().as_deref() {
                Some("dt") => terms.push(Node::Element(Element::new(
                    "dt",
                    self.phrasing(&child.children(), depth + 1),
                ))),
                Some("dd") => terms.push(Node::Element(Element::new(
                    "dd",
                    self.mixed(&child.children(), depth + 1),
                ))),
                // A term may be grouped with its definitions in a div.
                Some("div") => terms.extend(self.definitions(&child, depth + 1).children),
                _ => {}
            }
        }
        Element::new("dl", terms)
    }

    fn figure(&mut self, node: &NodeRef<'_>, depth: usize) -> Node {
        let mut content = Vec::new();
        let mut caption = None;
        for child in node.children() {
            if child.node_name().as_deref() == Some("figcaption") {
                // One line of caption, with a credit set in its own block
                // run on after it rather than broken out as a paragraph.
                let text = self.phrasing(&child.children(), depth + 1);
                if has_content(&text) {
                    caption = Some(Node::Element(Element::new("figcaption", text)));
                }
            } else {
                content.extend(self.mixed(std::slice::from_ref(&child), depth + 1));
            }
        }
        content.extend(caption);
        Node::Element(Element::new("figure", content))
    }

    fn table(&mut self, node: &NodeRef<'_>, depth: usize, out: &mut Vec<Node>) {
        let mut caption = None;
        let mut rows: Vec<NodeRef<'_>> = Vec::new();
        for child in node.children() {
            match child.node_name().as_deref() {
                Some("caption") => caption = Some(child),
                Some("tr") => rows.push(child),
                Some("thead" | "tbody" | "tfoot") => {
                    rows.extend(
                        child
                            .children()
                            .into_iter()
                            .filter(|row| row.node_name().as_deref() == Some("tr")),
                    );
                }
                _ => {}
            }
        }
        // A table of one column with no header is a page using a table for
        // layout; its cells are the text, read in order.
        let layout = rows.iter().all(|row| {
            let cells = cells(row);
            cells.len() <= 1
                && cells
                    .iter()
                    .all(|cell| cell.node_name().as_deref() == Some("td"))
        });
        if layout {
            for row in &rows {
                for cell in cells(row) {
                    out.extend(self.flow(&cell.children(), depth + 1));
                }
            }
            return;
        }
        let mut body = Vec::new();
        if let Some(caption) = caption {
            let text = self.phrasing(&caption.children(), depth + 1);
            if has_content(&text) {
                body.push(Node::Element(Element::new("caption", text)));
            }
        }
        let mut table_rows = Vec::new();
        for row in &rows {
            let mut row_cells = Vec::new();
            for cell in cells(row) {
                let header = cell.node_name().as_deref() == Some("th");
                let mut element = Element::new(
                    if header { "th" } else { "td" },
                    self.mixed(&cell.children(), depth + 2),
                );
                for span in ["colspan", "rowspan"] {
                    if let Some(count) =
                        number(cell.attr(span).as_deref()).filter(|count| *count > 1)
                    {
                        element.attributes.push((span, count.min(64).to_string()));
                    }
                }
                row_cells.push(Node::Element(element));
            }
            if !row_cells.is_empty() {
                table_rows.push(Node::Element(Element::new("tr", row_cells)));
            }
        }
        if table_rows.is_empty() {
            return;
        }
        body.push(Node::Element(Element::new("tbody", table_rows)));
        out.push(Node::Element(Element::new("table", body)));
    }

    /// Inline context.
    fn phrasing(&mut self, children: &[NodeRef<'_>], depth: usize) -> Vec<Node> {
        let mut out = Vec::new();
        for child in children {
            if child.is_text() {
                out.push(self.text(child));
            } else if child.is_element() {
                let name = child
                    .node_name()
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                if self.dropped(child, &name) {
                    continue;
                }
                if is_block(&name) {
                    // A block inside a line, which browsers allow: keep its
                    // words, set apart from the ones around them.
                    out.push(Node::Text(" ".to_owned()));
                    out.extend(self.phrasing(&child.children(), depth + 1));
                    out.push(Node::Text(" ".to_owned()));
                } else {
                    out.extend(self.inline(child, &name, depth + 1));
                }
            }
        }
        out
    }

    fn inline(&mut self, node: &NodeRef<'_>, name: &str, depth: usize) -> Vec<Node> {
        if depth > MAX_DEPTH {
            return vec![Node::Text(collapse_whitespace(&node.text()))];
        }
        let children = node.children();
        let wrap = |name: &'static str, children: Vec<Node>| {
            if has_content(&children) {
                vec![Node::Element(Element::new(name, children))]
            } else {
                children
            }
        };
        match name {
            "img" => self.picture(node, None).into_iter().collect(),
            "picture" => {
                let image = children
                    .iter()
                    .find(|child| child.node_name().as_deref() == Some("img"));
                match image {
                    Some(image) => self.picture(image, Some(node)).into_iter().collect(),
                    None => Vec::new(),
                }
            }
            "br" => vec![Node::Element(Element::new("br", Vec::new()))],
            "a" => {
                // A permalink marker beside a heading (#, ¶ or §) points at
                // the heading itself and says nothing.
                let marker = node
                    .attr("href")
                    .is_some_and(|href| href.trim().starts_with('#'))
                    && node.text().chars().all(|c| {
                        c.is_whitespace() || matches!(c, '#' | '\u{b6}' | '\u{a7}' | '\u{1f517}')
                    });
                if marker {
                    return Vec::new();
                }
                let inner = self.phrasing(&children, depth);
                let target = node
                    .attr("href")
                    .and_then(|href| {
                        let href = href.trim();
                        // A link within the page points at an anchor this
                        // book does not keep.
                        (!href.starts_with('#')).then(|| address::resolve(self.page, href))
                    })
                    .flatten();
                let only_pictures = inner.iter().all(|child| match child {
                    Node::Picture(_) => true,
                    Node::Text(text) => text.trim().is_empty(),
                    Node::Element(_) => false,
                });
                match target {
                    // A picture that links to a larger copy of itself is the
                    // picture; there is nothing to follow on a reader.
                    Some(target) if !only_pictures => {
                        let mut link = Element::new("a", inner);
                        link.attributes.push(("href", target));
                        vec![Node::Element(link)]
                    }
                    _ => inner,
                }
            }
            "em" | "i" | "dfn" | "var" => {
                let inner = self.phrasing(&children, depth);
                wrap("em", inner)
            }
            "strong" | "b" => {
                let inner = self.phrasing(&children, depth);
                wrap("strong", inner)
            }
            "code" | "kbd" | "samp" | "tt" => {
                self.code += 1;
                let inner = vec![Node::Text(node.text().to_string())];
                self.code -= 1;
                wrap("code", inner)
            }
            "sup" | "sub" | "small" => {
                let inner = self.phrasing(&children, depth);
                let name = match name {
                    "sup" => "sup",
                    "sub" => "sub",
                    _ => "small",
                };
                wrap(name, inner)
            }
            "q" => {
                let mut inner = vec![Node::Text("\u{201c}".to_owned())];
                inner.extend(self.phrasing(&children, depth));
                inner.push(Node::Text("\u{201d}".to_owned()));
                inner
            }
            "math" => {
                // A formula's written form, where the page gave one.
                let text = node
                    .attr("alttext")
                    .map_or_else(|| node.text().to_string(), |alt| alt.to_string());
                vec![Node::Text(collapse_whitespace(&text))]
            }
            _ => self.phrasing(&children, depth),
        }
    }

    /// The picture an `img` stands for, if it is one worth fetching.
    fn picture(&mut self, image: &NodeRef<'_>, picture: Option<&NodeRef<'_>>) -> Option<Node> {
        let width = number(image.attr("width").as_deref());
        let height = number(image.attr("height").as_deref());
        let alt = image
            .attr("alt")
            .map(|alt| collapse_whitespace(alt.trim()))
            .unwrap_or_default();
        let classes = image
            .attr("class")
            .map(|class| class.to_ascii_lowercase())
            .unwrap_or_default();
        // An emoji drawn as a picture is a character; keep the character.
        if words_of(&classes).any(|word| word == "emoji" || word == "smiley") {
            return (!alt.is_empty()).then_some(Node::Text(alt));
        }
        if words_of(&classes).any(|word| matches!(word.as_str(), "icon" | "avatar" | "logo")) {
            return None;
        }
        if width.is_some_and(|width| width <= 2) || height.is_some_and(|height| height <= 2) {
            return None;
        }
        let declared: Vec<u32> = [width, height].into_iter().flatten().collect();
        if !declared.is_empty() && declared.iter().all(|side| *side < ICON_SIDE) {
            return None;
        }

        // Every source the page offers, with the width each stands for.
        // Earlier entries win ties, so a lazy page's real source beats the
        // placeholder in `src`.
        let mut candidates: Vec<(String, u32)> = Vec::new();
        if let Some(picture) = picture {
            for source in picture.children() {
                if source.node_name().as_deref() != Some("source") {
                    continue;
                }
                let acceptable = source.attr("type").is_none_or(|kind| {
                    matches!(
                        kind.trim().to_ascii_lowercase().as_str(),
                        "image/jpeg" | "image/jpg" | "image/png" | "image/webp" | "image/gif"
                    )
                });
                if !acceptable {
                    continue;
                }
                for set in SOURCE_SETS {
                    if let Some(value) = source.attr(set) {
                        candidates.extend(source_set(&value, width));
                    }
                }
            }
        }
        for lazy in LAZY_SOURCES {
            if let Some(value) = image.attr(lazy) {
                candidates.push((value.trim().to_owned(), width.unwrap_or(1)));
            }
        }
        for set in SOURCE_SETS {
            if let Some(value) = image.attr(set) {
                candidates.extend(source_set(&value, width));
            }
        }
        if let Some(value) = image.attr("src") {
            candidates.push((value.trim().to_owned(), width.unwrap_or(1)));
        }

        let resolved: Vec<(String, u32)> = candidates
            .into_iter()
            .filter_map(|(source, width)| {
                address::resolve(self.page, &source).map(|address| (address, width))
            })
            .filter(|(address, _)| !unreadable_format(address) && !is_tracker(address))
            .collect();
        let large_enough = resolved
            .iter()
            .filter(|(_, width)| *width >= WANTED_WIDTH)
            .min_by_key(|(_, width)| *width);
        let chosen = large_enough.or_else(|| {
            // The widest, keeping the first of equals.
            resolved.iter().rev().max_by_key(|(_, width)| *width)
        })?;
        let address = chosen.0.clone();
        let known = self
            .pictures
            .iter()
            .position(|known| known.address == address);
        let index = known.unwrap_or_else(|| {
            self.pictures.push(Picture { address, alt });
            self.pictures.len() - 1
        });
        Some(Node::Picture(index))
    }
}

/// A table row's cells.
fn cells<'a>(row: &NodeRef<'a>) -> Vec<NodeRef<'a>> {
    row.children()
        .into_iter()
        .filter(|cell| matches!(cell.node_name().as_deref(), Some("td" | "th")))
        .collect()
}

/// Collects a `pre` block's text, keeping its line breaks.
fn preformatted(node: &NodeRef<'_>) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.is_text() {
            out.push_str(&child.text());
        } else if child.node_name().as_deref() == Some("br") {
            out.push('\n');
        } else if child.is_element() {
            out.push_str(&preformatted(&child));
        }
    }
    out.trim_matches('\n').to_owned()
}

/// Ends an inline run, as a paragraph in block context.
fn flush(run: &mut Vec<Node>, out: &mut Vec<Node>, wrap: bool) {
    if run.is_empty() {
        return;
    }
    let inline = std::mem::take(run);
    if wrap {
        push_paragraph(inline, out);
    } else if has_content(&inline) {
        out.extend(inline);
    }
}

/// Adds a paragraph, or the pictures alone when a paragraph held nothing
/// else, so a picture is never set inside an empty line.
fn push_paragraph(inline: Vec<Node>, out: &mut Vec<Node>) {
    if !has_content(&inline) {
        return;
    }
    let only_pictures = inline.iter().all(|node| match node {
        Node::Picture(_) => true,
        Node::Text(text) => text.trim().is_empty(),
        Node::Element(element) => element.name == "br",
    });
    if only_pictures {
        out.extend(
            inline
                .into_iter()
                .filter(|node| matches!(node, Node::Picture(_))),
        );
    } else {
        out.push(Node::Element(Element::new("p", inline)));
    }
}

/// Whether nodes hold any words or pictures.
fn has_content(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Text(text) => !text.trim().is_empty(),
        Node::Picture(_) => true,
        Node::Element(element) => element.name == "hr" || has_content(&element.children),
    })
}

/// Removes what the rebuild left empty and trims the edges of each block.
fn tidy(nodes: &mut Vec<Node>) {
    join_text(nodes);
    for node in nodes.iter_mut() {
        if let Node::Element(element) = node {
            if !matches!(element.name, "pre" | "code") {
                tidy(&mut element.children);
            }
            if matches!(
                element.name,
                "p" | "li"
                    | "dt"
                    | "dd"
                    | "td"
                    | "th"
                    | "figcaption"
                    | "caption"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
            ) {
                trim_edges(&mut element.children);
            }
        }
    }
    nodes.retain(|node| match node {
        Node::Text(_) | Node::Picture(_) => true,
        Node::Element(element) => {
            matches!(element.name, "br" | "hr" | "td" | "th") || has_content(&element.children)
        }
    });
    drop_empty_sections(nodes);
}

/// The rank of a heading, 2 for `h2` through 6, or none.
fn rank(node: &Node) -> Option<u8> {
    match node {
        Node::Element(element) if is_heading(element.name) => element.name[1..].parse().ok(),
        _ => None,
    }
}

/// Removes a heading with nothing under it: one followed directly by a
/// heading of the same or a higher rank, or by the end of the article. That
/// is what is left of a "Related stories" or "Comments" section once the
/// list under it has been taken out.
fn drop_empty_sections(nodes: &mut Vec<Node>) {
    let mut index = nodes.len();
    while index > 0 {
        index -= 1;
        let Some(level) = rank(&nodes[index]) else {
            continue;
        };
        let next = nodes[index + 1..]
            .iter()
            .find(|node| !matches!(node, Node::Text(text) if text.trim().is_empty()));
        let empty = match next {
            None => true,
            Some(next) => rank(next).is_some_and(|next| next <= level),
        };
        if empty {
            nodes.remove(index);
        }
    }
}

/// Joins neighbouring runs of text and collapses the spaces where they met,
/// so a block flattened into a line leaves one space behind, not three.
fn join_text(nodes: &mut Vec<Node>) {
    let mut joined: Vec<Node> = Vec::with_capacity(nodes.len());
    for node in nodes.drain(..) {
        match (joined.last_mut(), node) {
            (Some(Node::Text(before)), Node::Text(text)) => {
                if before.ends_with(' ') {
                    before.push_str(text.trim_start_matches(' '));
                } else {
                    before.push_str(&text);
                }
            }
            (_, node) => joined.push(node),
        }
    }
    *nodes = joined;
}

fn trim_edges(children: &mut Vec<Node>) {
    while matches!(children.first(), Some(Node::Text(text)) if text.trim().is_empty())
        || matches!(children.first(), Some(Node::Element(element)) if element.name == "br")
    {
        children.remove(0);
    }
    while matches!(children.last(), Some(Node::Text(text)) if text.trim().is_empty())
        || matches!(children.last(), Some(Node::Element(element)) if element.name == "br")
    {
        children.pop();
    }
    if let Some(Node::Text(text)) = children.first_mut() {
        *text = text.trim_start().to_owned();
    }
    if let Some(Node::Text(text)) = children.last_mut() {
        *text = text.trim_end().to_owned();
    }
}

/// Takes out the pictures that could not be used, and any figure or link
/// left holding nothing but a caption.
///
/// `usable` answers, for each picture index, the name it was stored under.
pub fn settle(nodes: &mut Vec<Node>, usable: &[Option<String>]) {
    for node in nodes.iter_mut() {
        if let Node::Element(element) = node {
            settle(&mut element.children, usable);
        }
    }
    nodes.retain(|node| match node {
        Node::Picture(index) => usable.get(*index).is_some_and(Option::is_some),
        Node::Text(_) => true,
        Node::Element(element) => match element.name {
            "br" | "hr" | "td" | "th" => true,
            "figure" => element.children.iter().any(|child| match child {
                Node::Element(inner) => inner.name != "figcaption" && has_content(&inner.children),
                Node::Picture(_) => true,
                Node::Text(text) => !text.trim().is_empty(),
            }),
            _ => has_content(&element.children),
        },
    });
}

/// Whether the body still holds a picture.
#[must_use]
pub fn has_picture(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node {
        Node::Picture(_) => true,
        Node::Element(element) => has_picture(&element.children),
        Node::Text(_) => false,
    })
}

/// Escapes text for XHTML content or a double-quoted attribute, dropping
/// the control characters XML has no spelling for.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(character),
            control if control < ' ' || control == '\u{7f}' => {}
            '\u{fffe}' | '\u{ffff}' => {}
            other => out.push(other),
        }
    }
    out
}

/// Writes nodes as XHTML. `names` gives each picture's file name, and
/// `pictures` its description.
pub fn render(nodes: &[Node], names: &[Option<String>], pictures: &[Picture], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(&escape(text)),
            Node::Picture(index) => {
                if let Some(Some(name)) = names.get(*index) {
                    let alt = pictures
                        .get(*index)
                        .map_or("", |picture| picture.alt.as_str());
                    let _ = write!(
                        out,
                        "<img src=\"{}\" alt=\"{}\"/>",
                        escape(name),
                        escape(alt)
                    );
                }
            }
            Node::Element(element) => {
                let _ = write!(out, "<{}", element.name);
                for (name, value) in &element.attributes {
                    let _ = write!(out, " {name}=\"{}\"", escape(value));
                }
                if matches!(element.name, "br" | "hr") {
                    out.push_str("/>");
                    if element.name == "hr" {
                        out.push('\n');
                    }
                    continue;
                }
                out.push('>');
                render(&element.children, names, pictures, out);
                let _ = write!(out, "</{}>", element.name);
                if is_block(element.name) {
                    out.push('\n');
                }
            }
        }
    }
}

/// Writes the body, setting a picture that stands alone between blocks in a
/// figure of its own.
pub fn render_body(body: &Body, names: &[Option<String>], out: &mut String) {
    for node in &body.nodes {
        if let Node::Picture(_) = node {
            out.push_str("<figure>");
            render(std::slice::from_ref(node), names, &body.pictures, out);
            out.push_str("</figure>\n");
        } else {
            render(std::slice::from_ref(node), names, &body.pictures, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://example.org/news/story.html";

    fn rebuilt(html: &str) -> Body {
        let document = dom_query::Document::from(html);
        let root = document.select("body");
        let root = *root.nodes().first().expect("a body");
        rebuild(&root, PAGE, "The Story", Some("Jane Doe"))
    }

    fn written(html: &str) -> String {
        let body = rebuilt(html);
        let names: Vec<Option<String>> = (0..body.pictures.len())
            .map(|index| Some(format!("images/{index}.jpg")))
            .collect();
        let mut out = String::new();
        render_body(&body, &names, &mut out);
        out
    }

    #[test]
    fn classes_styles_and_wrappers_are_dropped_and_the_words_kept() {
        let out = written(
            r#"<body><div class="wrap" style="color:red"><section><p class="lede">One <span class="x">two</span>.</p></section></div></body>"#,
        );
        assert_eq!(out, "<p>One two.</p>\n");
    }

    #[test]
    fn loose_text_between_blocks_becomes_a_paragraph() {
        let out = written("<body><div>Loose words<h2>Head</h2>more</div></body>");
        assert_eq!(out, "<p>Loose words</p>\n<h2>Head</h2>\n<p>more</p>\n");
    }

    #[test]
    fn a_heading_that_repeats_the_title_is_left_to_the_title_page() {
        let out = written("<body><h1>The  story</h1><h1>Part one</h1><p>Text.</p></body>");
        assert_eq!(out, "<h2>Part one</h2>\n<p>Text.</p>\n");
    }

    #[test]
    fn a_lazy_picture_is_given_its_real_source_rather_than_its_placeholder() {
        let body = rebuilt(
            r#"<body><p><img src="data:image/gif;base64,R0lGOD" data-src="/real.jpg" width="800" height="600"></p></body>"#,
        );
        assert_eq!(body.pictures[0].address, "https://example.org/real.jpg");
    }

    #[test]
    fn the_smallest_srcset_entry_as_wide_as_the_panel_is_chosen() {
        let body = rebuilt(
            r#"<body><img src="a-300.jpg" srcset="a-600.jpg 600w, a-1200.jpg 1200w, a-2400.jpg 2400w" width="300" height="200"></body>"#,
        );
        assert_eq!(
            body.pictures[0].address,
            "https://example.org/news/a-1200.jpg"
        );
    }

    #[test]
    fn a_density_srcset_takes_the_sharpest_copy_when_none_is_wide_enough() {
        let body = rebuilt(
            r#"<body><img src="t-250.jpg" srcset="t-500.jpg 2x" width="250" height="250"></body>"#,
        );
        assert_eq!(
            body.pictures[0].address,
            "https://example.org/news/t-500.jpg"
        );
    }

    #[test]
    fn a_picture_element_offers_its_sources_and_skips_formats_the_reader_cannot_use() {
        let body = rebuilt(
            r#"<body><picture><source type="image/avif" srcset="p.avif 1600w"><source type="image/webp" srcset="p.webp 1600w"><img src="p.jpg" width="400" height="300"></picture></body>"#,
        );
        assert_eq!(body.pictures[0].address, "https://example.org/news/p.webp");
    }

    #[test]
    fn tracking_pixels_icons_and_emoji_are_not_pictures() {
        let body = rebuilt(
            r#"<body><p>Hi <img class="emoji" alt="🙂" src="/e.png"> there<img src="/t.gif" width="1" height="1"><img src="/edit.png" width="15" height="15"><img src="https://px.example/pixel.gif?id=1"><img class="author-avatar" src="/me.jpg"></p></body>"#,
        );
        assert!(body.pictures.is_empty(), "{:?}", body.pictures);
        let mut out = String::new();
        render_body(&body, &[], &mut out);
        assert_eq!(out, "<p>Hi 🙂 there</p>\n");
    }

    #[test]
    fn a_figure_keeps_its_picture_and_caption_together() {
        let out = written(
            r#"<body><figure class="story-image"><a href="/big.jpg"><img src="/fig.jpg" alt="A tide chart" width="900" height="600"></a><figcaption>High water, <em>twice</em> a day.</figcaption></figure></body>"#,
        );
        assert_eq!(
            out,
            "<figure><img src=\"images/0.jpg\" alt=\"A tide chart\"/><figcaption>High water, <em>twice</em> a day.</figcaption>\n</figure>\n"
        );
    }

    #[test]
    fn a_figure_whose_picture_failed_leaves_no_orphaned_caption() {
        let mut body = rebuilt(
            r#"<body><p>Before.</p><figure><img src="/fig.jpg" width="900" height="600"><figcaption>Caption.</figcaption></figure><p>After.</p></body>"#,
        );
        settle(&mut body.nodes, &[None]);
        let mut out = String::new();
        render_body(&body, &[None], &mut out);
        assert_eq!(out, "<p>Before.</p>\n<p>After.</p>\n");
    }

    #[test]
    fn furniture_inside_the_article_is_left_out() {
        let out = written(
            r#"<body>
            <p>First paragraph.</p>
            <div class="share-bar"><a href="/tw">Share on X</a></div>
            <p>Advertisement</p>
            <div class="ad-slot">Buy things</div>
            <aside>Pull quote</aside>
            <p><strong>Read more:</strong> <a href="/other">Another story about tides entirely</a></p>
            <div class="newsletter-signup"><p>Sign up for our newsletter</p></div>
            <ul class="related-links"><li><a href="/1">One</a></li></ul>
            <p>Related: the tide was higher than forecast on Tuesday, as the harbour master noted.</p>
            </body>"#,
        );
        assert_eq!(
            out,
            "<p>First paragraph.</p>\n<p>Related: the tide was higher than forecast on Tuesday, as the harbour master noted.</p>\n"
        );
    }

    #[test]
    fn code_keeps_its_whitespace_and_its_highlighter_classes_do_not_hide_it() {
        let out = written(
            "<body><pre><code class=\"highlight\"><span class=\"highlight-comment\">// note</span>\nfn main() {\n    go();\n}</code></pre><p>Use <code>go()</code> &amp; <kbd>Enter</kbd>.</p></body>",
        );
        assert_eq!(
            out,
            "<pre>// note\nfn main() {\n    go();\n}</pre>\n<p>Use <code>go()</code> &amp; <code>Enter</code>.</p>\n"
        );
    }

    #[test]
    fn a_data_table_keeps_its_rows_and_a_layout_table_is_unwrapped() {
        let out = written(
            r#"<body><table><caption>Tides</caption><thead><tr><th>Day</th><th>High</th></tr></thead><tbody><tr><td>Mon</td><td colspan="2">06:10</td></tr></tbody></table><table><tr><td><p>Just text.</p></td></tr></table></body>"#,
        );
        assert_eq!(
            out,
            "<table><caption>Tides</caption>\n<tbody><tr><th>Day</th>\n<th>High</th>\n</tr>\n<tr><td>Mon</td>\n<td colspan=\"2\">06:10</td>\n</tr>\n</tbody>\n</table>\n<p>Just text.</p>\n"
        );
    }

    #[test]
    fn lists_and_quotes_keep_their_shape() {
        let out = written(
            r#"<body><ol start="3"><li>Three</li><li>Four<ul><li>Inner</li></ul></li></ol><blockquote>Said <q>so</q>.</blockquote></body>"#,
        );
        assert_eq!(
            out,
            "<ol start=\"3\"><li>Three</li>\n<li>Four<ul><li>Inner</li>\n</ul>\n</li>\n</ol>\n<blockquote><p>Said \u{201c}so\u{201d}.</p>\n</blockquote>\n"
        );
    }

    #[test]
    fn links_keep_their_full_address_and_links_within_the_page_are_unwrapped() {
        let out = written(
            r##"<body><p>See <a href="../other.html">this</a> and note <a href="#cite-1">[1]</a>.</p></body>"##,
        );
        assert_eq!(
            out,
            "<p>See <a href=\"https://example.org/other.html\">this</a> and note [1].</p>\n"
        );
    }

    #[test]
    fn a_heading_left_with_nothing_under_it_is_removed() {
        let out = written(
            r#"<body><h2>Tides</h2><h3>Spring</h3><p>Text.</p><h2>Related stories</h2><ul class="related"><li><a href="/a">A</a></li></ul><h2>Neaps</h2><p>More.</p><h2>Comments</h2></body>"#,
        );
        assert_eq!(
            out,
            "<h2>Tides</h2>\n<h3>Spring</h3>\n<p>Text.</p>\n<h2>Neaps</h2>\n<p>More.</p>\n"
        );
    }

    #[test]
    fn header_lines_the_title_page_already_says_are_left_out() {
        let out = written(
            r#"<body><p><a href="/news">News</a></p><p>The short version, in one line.</p><p>By <a href="/jane">Jane Doe</a></p><p>June 2, 2026</p><p>The story starts here and runs on for long enough that nobody could take it for a credit or a date line at the top of a page.</p><p>By Jane Doe</p></body>"#,
        );
        assert_eq!(
            out,
            "<p>The short version, in one line.</p>\n<p>The story starts here and runs on for long enough that nobody could take it for a credit or a date line at the top of a page.</p>\n<p>By Jane Doe</p>\n"
        );
    }

    #[test]
    fn hidden_elements_and_scripts_are_left_out() {
        let out = written(
            r#"<body><p>Kept.</p><p hidden>Hidden.</p><span aria-hidden="true">x</span><script>alert(1)</script><noscript>Enable JS</noscript></body>"#,
        );
        assert_eq!(out, "<p>Kept.</p>\n");
    }

    #[test]
    fn markup_characters_in_text_are_escaped() {
        let out = written("<body><p>a &lt; b &amp;&amp; c &gt; \"d\"</p></body>");
        assert_eq!(out, "<p>a &lt; b &amp;&amp; c &gt; &quot;d&quot;</p>\n");
    }
}
