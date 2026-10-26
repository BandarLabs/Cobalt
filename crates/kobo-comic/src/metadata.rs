//! A bounded subset of the `ComicInfo` interchange format, using Cobalt's XML scanner.

use std::io::{Cursor, Read};
use zip::ZipArchive;

const LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub series: Option<String>,
    pub number: Option<String>,
    pub right_to_left: Option<bool>,
    /// Zero-based index in the naturally sorted image pages, not ZIP file order.
    pub cover: Option<usize>,
    /// Optional metadata cannot make otherwise readable pages disappear.
    pub warning: Option<String>,
}

pub(super) fn read(archive: &mut ZipArchive<Cursor<&[u8]>>, pages: usize) -> Metadata {
    let names = archive
        .file_names()
        .filter(|name| name.eq_ignore_ascii_case("ComicInfo.xml"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let parsed = match names.as_slice() {
        [] => return Metadata::default(),
        [name] => read_one(archive, name, pages),
        _ => Err("More than one ComicInfo file was found."),
    };
    parsed.unwrap_or_else(|reason| Metadata {
        warning: Some(format!(
            "Comic details could not be read. {reason} The pages are still available."
        )),
        ..Metadata::default()
    })
}

fn read_one(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
    pages: usize,
) -> Result<Metadata, &'static str> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| "The details file is damaged.")?;
    if file.size() > LIMIT as u64 {
        return Err("The details file exceeds 64 KiB.");
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The details file is damaged.")?;
    if bytes.len() > LIMIT {
        return Err("The details file exceeds 64 KiB.");
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| "Use UTF-8 comic details.")?;
    parse(source, pages)
}

fn parse(source: &str, pages: usize) -> Result<Metadata, &'static str> {
    use kobo_xml::Event;
    if source.contains("<!DOCTYPE") || source.contains("<!ENTITY") {
        return Err("External definitions are not supported.");
    }
    let mut decoded = Vec::new();
    let mut events = Vec::new();
    kobo_xml::scan(source, &mut decoded, |event| events.push(event));
    let mut stack = Vec::new();
    let mut result = Metadata::default();
    let mut field_text = String::new();
    let mut closed = false;
    for event in events {
        match event {
            Event::Open { name, .. } => {
                if closed || (stack.is_empty() && name != "ComicInfo") {
                    return Err("The details file is not valid ComicInfo.");
                }
                if stack.len() == 1 {
                    field_text.clear();
                }
                if stack == ["ComicInfo", "Pages"]
                    && name == "Page"
                    && event.attribute("Type").as_deref() == Some("FrontCover")
                {
                    let cover = event
                        .attribute("Image")
                        .and_then(|v| v.parse::<usize>().ok())
                        .filter(|&i| i < pages)
                        .ok_or("The cover page is outside this comic.")?;
                    if result.cover.replace(cover).is_some() {
                        return Err("The comic names more than one front cover.");
                    }
                }
                stack.push(name);
            }
            Event::Close { name } => {
                if stack.last().copied() != Some(name) {
                    return Err("The details file is incomplete.");
                }
                if stack.len() == 2 {
                    assign(&mut result, name, &field_text)?;
                }
                stack.pop();
                if stack.is_empty() {
                    closed = true;
                }
            }
            Event::Text(text) => {
                append_text(&stack, text, &mut field_text)?;
            }
            Event::Owned(index) => {
                append_text(&stack, &decoded[index], &mut field_text)?;
            }
        }
    }
    if !closed || !stack.is_empty() {
        return Err("The details file is incomplete.");
    }
    Ok(result)
}

fn append_text(stack: &[&str], text: &str, output: &mut String) -> Result<(), &'static str> {
    if stack.is_empty() && !text.trim().is_empty() {
        return Err("The details file is not valid ComicInfo.");
    }
    if stack.len() == 2 && matches!(stack[1], "Title" | "Series" | "Number" | "Manga") {
        if output.len() + text.len() > 2048 {
            return Err("A comic detail is too long.");
        }
        output.push_str(text);
    }
    Ok(())
}

fn assign(metadata: &mut Metadata, name: &str, text: &str) -> Result<(), &'static str> {
    let value = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return Ok(());
    }
    let field = match name {
        "Title" => &mut metadata.title,
        "Series" => &mut metadata.series,
        "Number" => &mut metadata.number,
        "Manga" => {
            metadata.right_to_left = match value.as_str() {
                "YesAndRightToLeft" => Some(true),
                "Yes" | "No" => Some(false),
                _ => None,
            };
            return Ok(());
        }
        _ => return Ok(()),
    };
    if field.replace(value).is_some() {
        return Err("A comic detail was repeated.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unicode_title_direction_and_cover_without_reordering_pages() {
        let m = parse("<ComicInfo><Title>夜 &amp; Rain</Title><Series>Home</Series><Number>2</Number><Manga>YesAndRightToLeft</Manga><Pages><Page Image='1' Type='FrontCover'/></Pages></ComicInfo>", 3).unwrap();
        assert_eq!(m.title.as_deref(), Some("夜 & Rain"));
        assert_eq!(m.right_to_left, Some(true));
        assert_eq!(m.cover, Some(1));
    }
    #[test]
    fn refuses_incomplete_ambiguous_or_external_details() {
        for source in [
            "<ComicInfo><Title>x",
            "<ComicInfo><Title>x</Manga></ComicInfo>",
            "<!DOCTYPE ComicInfo SYSTEM 'file:///private'><ComicInfo/>",
            "<ComicInfo><Title>a</Title><Title>b</Title></ComicInfo>",
            "<ComicInfo><Pages><Page Image='9' Type='FrontCover'/></Pages></ComicInfo>",
        ] {
            assert!(parse(source, 2).is_err(), "{source}");
        }
    }
}
