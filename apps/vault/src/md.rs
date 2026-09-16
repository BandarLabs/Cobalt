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
pub fn render(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut html_out = String::new();
    html::push_html(&mut html_out, Parser::new_ext(markdown, options));
    kobo_html::to_text_within(&html_out, RENDER_CEILING)
}
