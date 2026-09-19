use pulldown_cmark::{html, Options, Parser};

/// The sole Markdown boundary. Normalised `CommonMark` becomes `HTML`, then
/// the platform's `HTML` text renderer, so a parser replacement stays in this
/// file. A note is a document somebody asked for by name, not a feed field,
/// so it converts against the shelf's own note ceiling rather than the
/// renderer's feed ceiling: a long note reaches its final sentence.
///
/// Mirrored from `shelf::MAX_NOTE_BYTES` (kept here so the path-included test
/// module compiles without the shelf codec).
const RENDER_CEILING: usize = 512 * 1024;
/// `[[target]]` and `[[target|label]]` read as their visible text; the
/// double-bracket syntax is an editing detail, not reading material.
fn wiki_links_to_text(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        if let Some(close) = rest[open + 2..].find("]]") {
            let inner = &rest[open + 2..open + 2 + close];
            let visible = inner.rsplit('|').next().unwrap_or(inner);
            out.push_str(visible);
            rest = &rest[open + 2 + close + 2..];
        } else {
            out.push_str(&rest[open..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

pub fn render(markdown: &str) -> String {
    let markdown = wiki_links_to_text(markdown);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut html_out = String::new();
    html::push_html(&mut html_out, Parser::new_ext(&markdown, options));
    kobo_html::to_text_within(&html_out, RENDER_CEILING)
}

#[cfg(test)]
mod tests {
    #[test]
    fn wiki_links_read_as_their_visible_text() {
        let rendered = super::render("See [[Projects/Alpha]] and [[Reading List|the list]].");
        assert!(rendered.contains("See Projects/Alpha and the list."));
        assert!(!rendered.contains("[["));
    }
}
