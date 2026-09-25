use pulldown_cmark::{html, Options, Parser};
/// The sole Markdown boundary. Normalised `CommonMark` becomes `HTML`, then the
/// platform's `HTML` text renderer, so a parser replacement stays in this file.
///
/// `ceiling` must be the size the body was actually fetched at ([`kobo_html::to_text_within`]'s
/// own requirement) -- an article is "a document somebody asked for by
/// name," not a summary field, so the default `kobo_html::to_text` (bounded
/// to 8 KiB, meant for a feed item among a thousand others) silently cut
/// every note over that size off mid-sentence with no sign anything was
/// missing.
pub fn render(markdown: &str, ceiling: usize) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let mut html_out = String::new();
    html::push_html(&mut html_out, Parser::new_ext(markdown, options));
    kobo_html::to_text_within(&html_out, ceiling)
}

#[cfg(test)]
mod tests {
    use super::render;

    /// The bug this pins: `kobo_html::to_text` (the field-bounded, 8 KiB
    /// conversion this module used before it took a ceiling) silently cut
    /// every note longer than that to a tenth of itself or less, mid
    /// sentence, with nothing to say content was missing. A real article
    /// easily clears 8 KiB of Markdown.
    #[test]
    fn a_body_longer_than_the_old_8kib_field_ceiling_survives_whole() {
        let paragraph = "The quick brown fox jumps over the lazy dog near the riverbank while the sun sets slowly behind the distant hills.\n\n";
        let mut markdown = String::new();
        while markdown.len() < 32 * 1024 {
            markdown.push_str(paragraph);
        }
        let rendered = render(&markdown, 1024 * 1024);
        assert!(
            rendered.len() > 16 * 1024,
            "rendered body was truncated to {} bytes",
            rendered.len()
        );
        assert!(rendered.trim_end().ends_with("hills."));
    }

    #[test]
    fn render_never_exceeds_the_ceiling_it_was_given() {
        let paragraph = "The quick brown fox jumps over the lazy dog.\n\n";
        let mut markdown = String::new();
        while markdown.len() < 64 * 1024 {
            markdown.push_str(paragraph);
        }
        let rendered = render(&markdown, 4096);
        assert!(rendered.len() <= 4096 + 4, "{}", rendered.len());
    }
}
