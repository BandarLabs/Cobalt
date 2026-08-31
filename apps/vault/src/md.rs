use pulldown_cmark::{html, Options, Parser};
/// The sole markdown boundary. Normalised CommonMark becomes HTML, then the
/// platform's HTML text renderer, so a parser replacement stays in this file.
pub fn render(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut html_out = String::new();
    html::push_html(&mut html_out, Parser::new_ext(markdown, options));
    kobo_html::to_text(&html_out)
}
