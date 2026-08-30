#![forbid(unsafe_code)]

use crate::model::superscript_verse_number;
use kobo_json::{parse, Value};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedChapter {
    pub translation: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter: u32,
    pub formatted_prose: String,
    pub verse_count: u32,
}

pub fn parse_chapter_json(json_str: &str) -> Result<ParsedChapter, &'static str> {
    let value = parse(json_str).map_err(|_| "Failed to parse JSON")?;

    let translation = value
        .get("translation")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("BSB")
        .to_string();

    let book_id = value
        .get("book")
        .and_then(|b| b.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("MRK")
        .to_string();

    let book_name = value
        .get("book")
        .and_then(|b| b.get("name").or_else(|| b.get("commonName")))
        .and_then(Value::as_str)
        .unwrap_or(&book_id)
        .to_string();

    let chapter_obj = value.get("chapter").ok_or("Missing chapter object")?;

    let chapter_num = chapter_obj
        .get("number")
        .and_then(Value::as_i64)
        .map(|n| n as u32)
        .unwrap_or(1);

    let content_array = chapter_obj
        .get("content")
        .and_then(Value::as_array)
        .ok_or("Missing chapter content array")?;

    let mut formatted_prose = String::with_capacity(8192);
    let mut verse_count = 0;
    let mut in_paragraph = false;

    for item in content_array {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "heading" => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        if in_paragraph {
                            formatted_prose.push_str("\n\n");
                            in_paragraph = false;
                        }
                        formatted_prose.push_str("— ");
                        formatted_prose.push_str(text);
                        formatted_prose.push_str(" —\n\n");
                    }
                }
            }
            "line_break" => {
                if in_paragraph {
                    formatted_prose.push_str("\n\n");
                    in_paragraph = false;
                }
            }
            "verse" => {
                let verse_num = item
                    .get("number")
                    .and_then(Value::as_i64)
                    .map(|n| n as u32)
                    .unwrap_or(0);
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        if !in_paragraph {
                            in_paragraph = true;
                        } else {
                            formatted_prose.push(' ');
                        }
                        if verse_num > 0 {
                            formatted_prose.push_str(&superscript_verse_number(verse_num));
                            formatted_prose.push(' ');
                            verse_count = verse_count.max(verse_num);
                        }
                        formatted_prose.push_str(text);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(ParsedChapter {
        translation,
        book_id,
        book_name,
        chapter: chapter_num,
        formatted_prose,
        verse_count,
    })
}
