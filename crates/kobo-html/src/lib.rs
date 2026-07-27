//! Turning a fragment of someone else's HTML into text a panel can draw.
//!
//! Two applications need this and neither controls what arrives. A Hacker News
//! comment body is HTML written by a stranger; an RSS item's description is
//! HTML written by a stranger and then passed through a publishing system that
//! may have escaped it once, twice, or not at all. Hacker News documents the
//! small set of tags it allows — `<p>`, `<i>`, `<a>`, `<pre><code>` — but that
//! is a statement about what a site *intends* to send, not a guarantee about
//! what turns up on the wire. This module is written for the other case.
//!
//! Three properties, each of which has a test:
//!
//! * It never panics. Every malformed shape a byte stream can take — a `<`
//!   with no `>`, a `&` with no `;`, tags inside tags, an entity that names a
//!   codepoint that does not exist — has a defined, boring outcome.
//! * It never grows. The output is bounded by [`MAX_TEXT`] and, below that
//!   ceiling, is never longer than the input: every construction it recognises
//!   is at least as long as what it produces. There is no input that makes it
//!   allocate a multiple of itself.
//! * It interprets nothing. The output is characters. Tags are removed rather
//!   than executed, entities are decoded exactly once and never rescanned, and
//!   control characters are dropped instead of being handed to a renderer that
//!   would have to decide what a `\u{7}` looks like.

/// The most text one converted body may produce.
///
/// An article summary is a few hundred words. This is far above that and far
/// below the point at which a page of them costs anything, and it exists so
/// that a single hostile field cannot make one screen's worth of state larger
/// than the whole response it came from.
pub const MAX_TEXT: usize = 8 * 1024;

/// The longest entity name this will consider, `&thetasym;` plus room.
///
/// A cap rather than a search to the end of the input: without one, a body
/// consisting of a single `&` followed by half a megabyte of letters is half a
/// megabyte of scanning for every `&` in it.
const MAX_ENTITY: usize = 12;

/// Elements whose *content* is dropped along with their tags.
///
/// Neither source is supposed to send these. If one arrives, the text inside
/// it was written to be executed rather than read, and rendering `alert(1)` as
/// prose is not useful to anybody. Nothing here could run it — the output of
/// this module is characters on an E Ink panel — but it is still noise, and
/// noise that says something went wrong upstream.
const OPAQUE: [&str; 2] = ["script", "style"];

/// The named entities that appear in practice, with their replacements.
///
/// Deliberately short. The full HTML5 table is two thousand entries; Hacker
/// News uses six of them, and an unknown name is left alone rather than
/// guessed at, so a name missing from here is displayed rather than lost.
const NAMED: [(&str, &str); 12] = [
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", " "),
    ("hellip", "\u{2026}"),
    ("mdash", "\u{2014}"),
    ("ndash", "\u{2013}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("ldquo", "\u{201c}"),
];

/// Converts one HTML fragment into plain text with blank lines between
/// paragraphs.
///
/// `<p>` becomes a paragraph break, every other tag is removed, and entities
/// are decoded. The result is what [`kobo_sdk::Context::paginate`] expects:
/// paragraphs separated by a blank line, with no markup left in them.
#[must_use]
pub fn to_text(html: &str) -> String {
    // Never larger than the input, and never larger than the ceiling. Both
    // bounds matter: the first is what makes the conversion safe on a body
    // that is already at the transport limit, the second is what makes it safe
    // on a body that arrives inside a thread of a thousand others.
    let mut out = String::with_capacity(html.len().min(MAX_TEXT));
    let mut rest = html;
    while let Some(at) = rest.find(['<', '&']) {
        // `<` and `&` are ASCII, so the offset is always a character boundary
        // and both slices below are valid however the rest of the input is
        // encoded.
        push_text(&mut out, &rest[..at]);
        if out.len() >= MAX_TEXT {
            break;
        }
        let tail = &rest[at..];
        rest = if tail.starts_with('<') {
            take_tag(&mut out, tail)
        } else {
            take_entity(&mut out, tail)
        };
    }
    if out.len() < MAX_TEXT {
        push_text(&mut out, rest);
    }
    if out.len() >= MAX_TEXT {
        // Cut on a character boundary, then say so. Silently stopping mid-word
        // reads as a comment whose author trailed off.
        let mut cut = MAX_TEXT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('\u{2026}');
    }
    out.trim().to_owned()
}

/// Appends literal text, dropping control characters.
///
/// A stray `\u{7}` or `\u{1b}` in a comment has no drawing, and letting one
/// through means every renderer downstream has to decide what to do with it.
/// Tabs and newlines are kept because the paginator understands both.
fn push_text(out: &mut String, text: &str) {
    // Space that follows a paragraph break is indentation somebody wrote for a
    // browser, and the paginator emits one node per paragraph: keeping it
    // pushes the first word of a paragraph off its own left margin.
    let text = if out.ends_with('\n') {
        text.trim_start()
    } else {
        text
    };
    for character in text.chars() {
        if out.len() >= MAX_TEXT {
            return;
        }
        if character == '\n' || character == '\t' || !character.is_control() {
            out.push(character);
        }
    }
}

/// Consumes one tag from the front of `tail`, which begins with `<`.
///
/// Returns what is left afterwards. A `<` that never closes is not a tag at
/// all: it is emitted as the character it is and scanning continues one byte
/// later, which is the only reading that cannot lose the rest of a comment.
fn take_tag<'a>(out: &mut String, tail: &'a str) -> &'a str {
    let Some(end) = tail.find('>') else {
        out.push('<');
        return &tail[1..];
    };
    let inside = &tail[1..end];
    let after = &tail[end + 1..];
    let name = element_name(inside);
    if breaks_paragraph(&name) {
        push_break(out);
    }
    if OPAQUE.contains(&name.as_str()) && !inside.starts_with('/') {
        return skip_element(after, &name);
    }
    after
}

/// The lower-case element name inside a tag body, without its attributes.
///
/// `/p` and `a href="…"` both reduce to something this can compare, and a tag
/// body containing another `<` — which is not a tag, but is a thing a stranger
/// can send — reduces to a name that matches nothing.
fn element_name(inside: &str) -> String {
    inside
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/' || c == '<')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Whether this element starts a new paragraph on the panel.
///
/// Hacker News writes `<p>` with no closing tag, so the *opening* one is the
/// break. `pre` is here because a code block that ran on from the sentence
/// before it would be unreadable; `br` is not, because a single newline is
/// folded into a space by the paginator, which is what a soft break should do.
fn breaks_paragraph(name: &str) -> bool {
    matches!(name, "p" | "pre" | "blockquote" | "li" | "div")
}

/// Adds a paragraph break unless there is already one, or nothing yet.
fn push_break(out: &mut String) {
    if out.is_empty() || out.ends_with("\n\n") || out.len() + 2 > MAX_TEXT {
        return;
    }
    while out.ends_with(' ') || out.ends_with('\n') {
        out.pop();
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
}

/// Skips to the end of an element whose content is not text.
///
/// Falls off the end of the input when the close tag never arrives, which is
/// the safe direction: an unterminated `<script>` swallows the rest of the
/// comment rather than spilling its body onto the panel.
fn skip_element<'a>(after: &'a str, name: &str) -> &'a str {
    let mut rest = after;
    while let Some(at) = rest.find('<') {
        let candidate = &rest[at..];
        let Some(end) = candidate.find('>') else {
            return "";
        };
        if candidate[1..].starts_with('/') && element_name(&candidate[1..end]) == name {
            return &candidate[end + 1..];
        }
        rest = &candidate[end + 1..];
    }
    ""
}

/// Consumes one entity from the front of `tail`, which begins with `&`.
///
/// Anything that is not a complete, known entity is left as the `&` it was:
/// `AT&T` is a company, not a broken reference, and mangling it would be a
/// worse outcome than not decoding something.
fn take_entity<'a>(out: &mut String, tail: &'a str) -> &'a str {
    let body = &tail[1..];
    // A byte search over a fixed-length prefix rather than a search of the
    // whole remainder. `;` cannot occur inside a multi-byte character, so the
    // position it finds is always a character boundary, and the prefix is what
    // stops a body of one `&` and half a megabyte of letters costing a scan of
    // half a megabyte for every ampersand in it.
    let head = &body.as_bytes()[..MAX_ENTITY.min(body.len())];
    let Some(end) = head.iter().position(|byte| *byte == b';') else {
        out.push('&');
        return body;
    };
    let name = &body[..end];
    let after = &body[end + 1..];
    if let Some(digits) = name.strip_prefix('#') {
        // A reference to a codepoint that does not exist, or to one with no
        // drawing, is left exactly as written. Substituting U+FFFD would put a
        // black diamond in the middle of a sentence and blame the panel for
        // it; dropping it silently would hide that it was ever there.
        let Some(character) = numeric(digits) else {
            out.push('&');
            return body;
        };
        out.push(character);
        return after;
    }
    let Some((_, replacement)) = NAMED
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
    else {
        out.push('&');
        return body;
    };
    out.push_str(replacement);
    after
}

/// Decodes `#39` or `#x27`, the two spellings Hacker News uses.
///
/// Returns `None` for anything that is not a drawable character, which covers
/// an empty reference, one too long to be a codepoint, one that names a
/// surrogate half, and one that names a control character.
fn numeric(digits: &str) -> Option<char> {
    let (digits, radix) = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => (hex, 16),
        None => (digits, 10),
    };
    // Eight digits is past the top of Unicode in either base, so the parse
    // below can never see a number large enough to be interesting.
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let code = u32::from_str_radix(digits, radix).ok()?;
    let character = char::from_u32(code)?;
    if character.is_control() && character != '\n' && character != '\t' {
        return None;
    }
    Some(character)
}

#[cfg(test)]
mod tests {
    use super::{to_text, MAX_TEXT};

    #[test]
    fn a_paragraph_tag_becomes_a_blank_line_and_everything_else_is_removed() {
        // Hacker News writes `<p>` with no closing tag, so the opening one is
        // the break. Losing it turns a three paragraph comment into one wall.
        assert_eq!(
            to_text("First thought.<p>Second thought.<p>Third."),
            "First thought.\n\nSecond thought.\n\nThird."
        );
        assert_eq!(
            to_text("A <i>stressed</i> word and <a href=\"https://x.test/\">a link</a>."),
            "A stressed word and a link."
        );
    }

    #[test]
    fn the_entities_hacker_news_actually_sends_are_decoded() {
        // These six account for essentially every entity in a real thread;
        // leaving them encoded puts `&#x27;` in the middle of every
        // contraction on the panel.
        assert_eq!(
            to_text("it&#x27;s &quot;fine&quot; &amp; 1 &lt; 2 &gt; 0, see &#x2F;usr&#x2F;bin"),
            "it's \"fine\" & 1 < 2 > 0, see /usr/bin"
        );
        assert_eq!(to_text("&#39;&#8212;&#x1F600;"), "'\u{2014}\u{1f600}");
    }

    #[test]
    fn a_decoded_entity_is_never_decoded_a_second_time() {
        // The attack this closes: `&amp;lt;` decoding to `&lt;` and then to
        // `<` would let a comment reintroduce markup that the site escaped on
        // the way out. Output is written, never rescanned.
        assert_eq!(to_text("&amp;lt;script&amp;gt;"), "&lt;script&gt;");
        assert_eq!(to_text("&amp;amp;"), "&amp;");
    }

    #[test]
    fn a_tag_that_never_closes_does_not_swallow_the_comment() {
        // A lone `<` is a character somebody typed, not the start of markup.
        // Treating it as a tag would delete everything after it.
        assert_eq!(to_text("if x < y then say so"), "if x < y then say so");
        assert_eq!(to_text("a <b"), "a <b");
        assert_eq!(to_text("<"), "<");
        assert_eq!(to_text("<<<<"), "<<<<");
    }

    #[test]
    fn an_ampersand_that_names_nothing_survives_as_itself() {
        // `AT&T` is a company. So is `Procter&Gamble`. Neither is malformed
        // input, and neither should lose a character.
        assert_eq!(to_text("AT&T and P&G"), "AT&T and P&G");
        assert_eq!(to_text("&"), "&");
        assert_eq!(to_text("&notarealentity; x"), "&notarealentity; x");
        assert_eq!(to_text("&;"), "&;");
        // Past the scan ceiling, so not an entity however it is spelled.
        assert_eq!(
            to_text("&averyverylongname; y"),
            "&averyverylongname; y",
            "an over-long name was decoded"
        );
    }

    #[test]
    fn nested_and_malformed_angle_brackets_terminate() {
        // Every one of these is a shape a stranger can send and none of them
        // has an obvious reading. What matters is that each has *a* reading
        // and that none of them is a panic or a loop.
        assert_eq!(to_text("<a <b>>text"), ">text");
        assert_eq!(to_text("<<p>>"), ">");
        assert_eq!(to_text("<p"), "<p");
        assert_eq!(to_text("</>"), "");
        assert_eq!(to_text("<p></p></p></p>"), "");
        assert_eq!(to_text("a<p><p><p>b"), "a\n\nb");
    }

    #[test]
    fn a_reference_to_a_character_that_cannot_be_drawn_is_dropped() {
        // A NUL, an escape and a lone surrogate half all name something with
        // no glyph. Emitting them hands the renderer a decision it should
        // never have to make, and mangling the text around them would hide
        // that the reference was there at all.
        assert_eq!(to_text("a&#0;b"), "a&#0;b");
        assert_eq!(to_text("a&#x1b;b"), "a&#x1b;b");
        assert_eq!(to_text("a&#xD800;b"), "a&#xD800;b");
        assert_eq!(to_text("a&#999999999999;b"), "a&#999999999999;b");
        assert_eq!(to_text("a&#;b"), "a&#;b");
        assert_eq!(to_text("a&#x;b"), "a&#x;b");
        assert_eq!(to_text("a\u{7}b"), "ab");
    }

    #[test]
    fn script_and_style_lose_their_contents_as_well_as_their_tags() {
        // Nothing here could run it, but a comment whose text is `alert(1)`
        // is not a comment, and an unterminated one must not spill.
        assert_eq!(
            to_text("before<script>alert(1)</script>after"),
            "beforeafter"
        );
        assert_eq!(to_text("before<style>b{x:y}</style>after"), "beforeafter");
        assert_eq!(to_text("before<script>alert(1)"), "before");
        assert_eq!(to_text("before<script>a<b>c</script>after"), "beforeafter");
    }

    #[test]
    fn the_output_is_never_longer_than_the_input_and_never_past_the_ceiling() {
        // The property that makes this safe to run over a thousand comments:
        // there is no fragment that expands. If one existed, a body at the
        // transport ceiling could become several times the response it
        // arrived in.
        for hostile in [
            "<p>".repeat(4000),
            "&".repeat(4000),
            "&#x27;".repeat(4000),
            "<".repeat(4000),
            "&amp;".repeat(4000),
            "<p><i><a href=\"#\">".repeat(400),
        ] {
            let text = to_text(&hostile);
            assert!(
                text.len() <= hostile.len(),
                "{} bytes in, {} out",
                hostile.len(),
                text.len()
            );
            assert!(
                text.len() <= MAX_TEXT + 4,
                "past the ceiling: {}",
                text.len()
            );
        }
    }

    #[test]
    fn a_body_past_the_ceiling_is_cut_on_a_character_boundary_and_says_so() {
        // Cutting inside a multi-byte character would panic; cutting without
        // saying so reads as an author who trailed off.
        let long = "\u{e9}".repeat(MAX_TEXT);
        let text = to_text(&long);
        assert!(text.ends_with('\u{2026}'), "no mark that it was cut");
        assert!(text.len() <= MAX_TEXT + 4);
        assert!(text.chars().count() > 1000, "cut far too early");
    }

    #[test]
    fn a_code_block_stands_apart_from_the_sentence_before_it() {
        // Hacker News wraps code in `<pre><code>`, and running it on from the
        // paragraph above turns both into nonsense.
        assert_eq!(
            to_text("Try this:<pre><code>  cargo build\n</code></pre>then read it."),
            "Try this:\n\ncargo build\n\nthen read it."
        );
    }

    #[test]
    fn whitespace_around_a_break_does_not_become_an_empty_paragraph() {
        // The paginator emits one text node per paragraph, so a blank one is a
        // visible gap the author did not write.
        assert_eq!(to_text("<p>leading break"), "leading break");
        assert_eq!(to_text("trailing break<p>"), "trailing break");
        assert_eq!(to_text("a  <p>  b"), "a\n\nb");
        assert_eq!(to_text(""), "");
        assert_eq!(to_text("   "), "");
    }
}
