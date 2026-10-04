//! What the bridge sends, read into plain values.
//!
//! The bridge has already turned Muse's markdown into blocks, so nothing here
//! parses markup. Anything malformed is dropped as a whole reply rather than
//! half drawn.

use kobo_json::Value;

/// A run of text, bold or not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    pub text: String,
    pub strong: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    List(Vec<Vec<Span>>),
    Quote(Vec<Span>),
    Rule,
    Image { blob: String, alt: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub context: String,
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Picture {
    pub blob: String,
    pub fill: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Content {
    Status,
    Page { title: String, blocks: Vec<Block> },
    Ask(Question),
    Image(Picture),
}

/// One poll's worth of screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Live {
    pub rev: u64,
    pub line: String,
    pub detail: String,
    pub content: Content,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reply {
    Unchanged(u64),
    Changed(Live),
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn spans(value: &Value) -> Option<Vec<Span>> {
    let items = value.as_array()?;
    let out: Vec<Span> = items
        .iter()
        .map(|item| Span {
            text: text(item, "s"),
            strong: item.get("b").and_then(Value::as_bool) == Some(true),
        })
        .collect();
    Some(out)
}

fn block(value: &Value) -> Option<Block> {
    Some(match value.get("t")?.as_str()? {
        "h" => Block::Heading {
            level: if value.get("level").and_then(Value::as_i64) == Some(1) {
                1
            } else {
                2
            },
            spans: spans(value.get("spans")?)?,
        },
        "p" => Block::Paragraph(spans(value.get("spans")?)?),
        "q" => Block::Quote(spans(value.get("spans")?)?),
        "ul" => Block::List(
            value
                .get("items")?
                .as_array()?
                .iter()
                .map(spans)
                .collect::<Option<Vec<_>>>()?,
        ),
        "hr" => Block::Rule,
        "img" => Block::Image {
            blob: text(value, "blob"),
            alt: text(value, "alt"),
        },
        _ => return None,
    })
}

fn question(value: &Value) -> Option<Question> {
    let choices: Vec<Choice> = value
        .get("choices")?
        .as_array()?
        .iter()
        .map(|choice| Choice {
            id: text(choice, "id"),
            label: text(choice, "label"),
        })
        .filter(|choice| !choice.id.is_empty() && !choice.label.is_empty())
        .collect();
    let id = text(value, "ask_id");
    if id.is_empty() || choices.len() < 2 {
        return None;
    }
    Some(Question {
        id,
        text: text(value, "question"),
        context: text(value, "context"),
        choices,
    })
}

/// Reads a `/v1/screen` reply. `None` means the bytes were not one.
pub fn read(bytes: &[u8]) -> Option<Reply> {
    let body = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let rev = u64::try_from(body.get("rev")?.as_i64()?).ok()?;
    if body.get("unchanged").and_then(Value::as_bool) == Some(true) {
        return Some(Reply::Unchanged(rev));
    }
    let status = body.get("status")?;
    let content = match body.get("kind")?.as_str()? {
        "status" => Content::Status,
        "page" => {
            let page = body.get("page")?;
            Content::Page {
                title: text(page, "title"),
                blocks: page
                    .get("blocks")?
                    .as_array()?
                    .iter()
                    .map(block)
                    .collect::<Option<Vec<_>>>()?,
            }
        }
        "ask" => Content::Ask(question(body.get("ask")?)?),
        "image" => {
            let image = body.get("image")?;
            Content::Image(Picture {
                blob: text(image, "blob"),
                fill: image.get("fit").and_then(Value::as_str) == Some("fill"),
            })
        }
        _ => return None,
    };
    Some(Reply::Changed(Live {
        rev,
        line: text(status, "line"),
        detail: text(status, "detail"),
        content,
    }))
}

/// The plain text of a run of spans.
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_is_just_the_rev() {
        assert_eq!(
            read(br#"{"rev":4,"unchanged":true}"#),
            Some(Reply::Unchanged(4))
        );
    }

    #[test]
    fn a_page_reads_into_blocks() {
        let reply = read(
            br#"{"rev":9,"kind":"page","status":{"line":"Idle","detail":""},
            "page":{"id":"p","title":"Today","blocks":[
              {"t":"h","level":1,"spans":[{"s":"Plan"}]},
              {"t":"p","spans":[{"s":"Lunch "},{"s":"at 1","b":true}]},
              {"t":"ul","items":[[{"s":"Gym"}],[{"s":"Read"}]]},
              {"t":"hr"}]}}"#,
        );
        let Some(Reply::Changed(live)) = reply else {
            panic!("a changed screen");
        };
        let Content::Page { title, blocks } = live.content else {
            panic!("a page");
        };
        assert_eq!(title, "Today");
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[3], Block::Rule);
        let Block::Paragraph(runs) = &blocks[1] else {
            panic!("a paragraph");
        };
        assert!(runs[1].strong && plain(runs) == "Lunch at 1");
    }

    #[test]
    fn an_ask_needs_an_id_and_two_choices() {
        let ask = br#"{"rev":2,"kind":"ask","status":{"line":"","detail":""},
            "ask":{"ask_id":"a1","question":"Go?","context":"","choices":[
              {"id":"c1","label":"Yes"},{"id":"c2","label":"No"}]}}"#;
        assert!(matches!(
            read(ask),
            Some(Reply::Changed(Live {
                content: Content::Ask(_),
                ..
            }))
        ));
        let one = br#"{"rev":2,"kind":"ask","status":{"line":"","detail":""},
            "ask":{"ask_id":"a1","question":"Go?","choices":[{"id":"c1","label":"Yes"}]}}"#;
        assert_eq!(read(one), None);
    }

    #[test]
    fn nonsense_is_refused_whole() {
        assert_eq!(read(b"<html>"), None);
        assert_eq!(read(br#"{"rev":1,"kind":"nope","status":{}}"#), None);
        assert_eq!(
            read(br#"{"rev":1,"kind":"page","status":{},"page":{"blocks":[{"t":"zz"}]}}"#),
            None
        );
    }
}
