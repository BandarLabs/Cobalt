//! Bounded AO3 parsing and the durable shelf catalogue.

use kobo_xml::{decode_entities, scan, split_name, Event};
use std::fmt::Write as _;

pub const MAX_WORKS: usize = 96;
pub const MAX_TAGS: usize = 24;
pub const MAX_FEED_WORKS: usize = 24;

const TITLE_MAX: usize = 180;
const AUTHOR_MAX: usize = 140;
const FANDOM_MAX: usize = 180;
const RATING_MAX: usize = 80;
const WARNINGS_MAX: usize = 360;
const SUMMARY_MAX: usize = 520;
const DATE_MAX: usize = 32;
const URL_MAX: usize = 768;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DownloadState {
    #[default]
    NotDownloaded,
    Downloaded,
    UpdateAvailable,
    Removed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Work {
    pub id: String,
    pub title: String,
    pub author: String,
    pub fandom: String,
    pub rating: String,
    pub warnings: String,
    pub summary: String,
    pub chapters: u16,
    pub total_chapters: Option<u16>,
    pub complete: bool,
    pub updated: String,
    pub epub: String,
    pub download: DownloadState,
    pub adult: bool,
}

impl Work {
    pub fn chapters_label(&self) -> String {
        format!(
            "{}/{}{}",
            self.chapters,
            self.total_chapters
                .map_or_else(|| "?".to_owned(), |total| total.to_string()),
            if self.complete { " complete" } else { " WIP" }
        )
    }

    pub const fn downloaded(&self) -> bool {
        matches!(
            self.download,
            DownloadState::Downloaded | DownloadState::UpdateAvailable | DownloadState::Removed
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FollowedTag {
    pub name: String,
    pub slug: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FeedWork {
    pub id: String,
    pub title: String,
    pub author: String,
    pub updated: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedWork {
    Work(Box<Work>),
    AdultInterstitial,
    Locked,
    Missing,
    Malformed,
}

pub fn shelf_name(id: &str) -> String {
    format!("work-{id}.epub")
}

pub fn place_key(id: &str) -> String {
    format!("place.{id}")
}

pub fn work_url(id: &str, adult: bool) -> String {
    let suffix = if adult { "?view_adult=true" } else { "" };
    format!("https://archiveofourown.org/works/{id}{suffix}")
}

pub fn feed_url(tag: &FollowedTag) -> String {
    format!("https://archiveofourown.org/tags/{}/feeds.atom", tag.slug)
}

pub fn parse_work_page(id: &str, body: &str) -> ParsedWork {
    let lower = body.to_ascii_lowercase();
    if lower.contains("view_adult=true")
        && (lower.contains("adult content") || lower.contains("proceed"))
    {
        return ParsedWork::AdultInterstitial;
    }
    if lower.contains("only available to registered users")
        || lower.contains("you need to log in to access this work")
    {
        return ParsedWork::Locked;
    }
    if lower.contains("error 404") || lower.contains("couldn't find the work") {
        return ParsedWork::Missing;
    }

    let title = class_text(body, "title heading", TITLE_MAX)
        .or_else(|| element_text(body, "title", TITLE_MAX))
        .map(|title| title.replace(" | Archive of Our Own", ""))
        .map(|title| bounded(&title, TITLE_MAX))
        .unwrap_or_default();
    let author = attribute_text(body, "rel", "author", AUTHOR_MAX)
        .or_else(|| class_text(body, "byline heading", AUTHOR_MAX))
        .unwrap_or_else(|| "Anonymous".to_owned());
    let fandom =
        class_text(body, "fandom tags", FANDOM_MAX).unwrap_or_else(|| "Unspecified".into());
    let rating = class_text(body, "rating tags", RATING_MAX).unwrap_or_else(|| "Not Rated".into());
    let warnings = class_text(body, "warning tags", WARNINGS_MAX)
        .unwrap_or_else(|| "Creator Chose Not To Use Archive Warnings".into());
    let summary = class_text(body, "summary module", SUMMARY_MAX).unwrap_or_default();
    let updated = class_text(body, "updated", DATE_MAX)
        .or_else(|| class_text(body, "published", DATE_MAX))
        .unwrap_or_default();
    let chapter_text = class_text(body, "chapters", 32).unwrap_or_else(|| "1/1".into());
    let (chapters, total_chapters) = parse_chapters(&chapter_text);
    let complete = total_chapters.is_some_and(|total| chapters >= total);
    let epub = epub_url(body).unwrap_or_default();

    if title.trim().is_empty() || epub.is_empty() {
        return ParsedWork::Malformed;
    }
    ParsedWork::Work(Box::new(Work {
        id: id.to_owned(),
        title,
        author,
        fandom,
        rating,
        warnings,
        summary,
        chapters,
        total_chapters,
        complete,
        updated,
        epub,
        download: DownloadState::NotDownloaded,
        adult: false,
    }))
}

fn parse_chapters(text: &str) -> (u16, Option<u16>) {
    let mut parts = text.trim().split('/');
    let current = parts.next().and_then(number).unwrap_or(1);
    let total = parts.next().and_then(number);
    (current, total)
}

fn number(text: &str) -> Option<u16> {
    let digits = text
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

fn epub_url(body: &str) -> Option<String> {
    let mut rest = body;
    while let Some(at) = rest.find("href=") {
        rest = &rest[at + 5..];
        let quote = rest.chars().next()?;
        if !matches!(quote, '"' | '\'') {
            continue;
        }
        rest = &rest[1..];
        let end = rest.find(quote)?;
        let href = decode_entities(&rest[..end]);
        rest = &rest[end + 1..];
        if !href.contains("/downloads/") || !href.contains(".epub") {
            continue;
        }
        let absolute = if href.starts_with("https://archiveofourown.org/") {
            href
        } else if href.starts_with('/') {
            format!("https://archiveofourown.org{href}")
        } else {
            continue;
        };
        return Some(bounded(&absolute, URL_MAX));
    }
    None
}

fn class_text(body: &str, class: &str, limit: usize) -> Option<String> {
    attribute_text(body, "class", class, limit)
}

fn attribute_text(body: &str, attribute: &str, wanted: &str, limit: usize) -> Option<String> {
    let mut rest = body;
    while let Some(at) = rest.find('<') {
        rest = &rest[at..];
        let end = rest.find('>')?;
        let head = &rest[..=end];
        if attribute_value(head, attribute).is_some_and(|value| {
            value == wanted
                || (attribute == "class"
                    && wanted
                        .split_whitespace()
                        .all(|name| value.split_whitespace().any(|part| part == name)))
        }) {
            let tag = head[1..]
                .trim_start_matches('/')
                .split(|character: char| character.is_whitespace() || character == '>')
                .next()?;
            let close = format!("</{tag}>");
            let tail = &rest[end + 1..];
            let close_at = tail.find(&close)?;
            return Some(plain(&tail[..close_at], limit));
        }
        rest = &rest[end + 1..];
    }
    None
}

fn element_text(body: &str, tag: &str, limit: usize) -> Option<String> {
    let open = format!("<{tag}");
    let at = body.find(&open)?;
    let tail = &body[at..];
    let head_end = tail.find('>')?;
    let close = format!("</{tag}>");
    let end = tail[head_end + 1..].find(&close)?;
    Some(plain(&tail[head_end + 1..head_end + 1 + end], limit))
}

fn attribute_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=");
    let at = head.find(&marker)?;
    let tail = &head[at + marker.len()..];
    let quote = tail.chars().next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let tail = &tail[1..];
    Some(&tail[..tail.find(quote)?])
}

fn plain(html: &str, limit: usize) -> String {
    let mut text = String::with_capacity(html.len().min(limit));
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(character),
            _ => {}
        }
        if text.len() >= limit.saturating_mul(4) {
            break;
        }
    }
    let decoded = decode_entities(&text);
    let compact = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    bounded(&compact, limit)
}

fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.trim().to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text[..end].trim().to_owned()
}

pub fn parse_tag(input: &str) -> Option<FollowedTag> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (name, slug) = if let Some(tail) = input.split("/tags/").nth(1) {
        let slug = tail
            .split('/')
            .next()
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default();
        (decode_percent(slug), slug.to_owned())
    } else {
        (input.to_owned(), encode_path(input))
    };
    (!slug.is_empty() && slug.len() <= 512).then(|| FollowedTag {
        name: bounded(&name, 160),
        slug,
    })
}

fn encode_path(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn decode_percent(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let raw = text.as_bytes();
    let mut at = 0;
    while at < raw.len() {
        if raw[at] == b'%' && at + 2 < raw.len() {
            if let (Some(high), Some(low)) = (
                char::from(raw[at + 1]).to_digit(16),
                char::from(raw[at + 2]).to_digit(16),
            ) {
                bytes.push(u8::try_from((high << 4) | low).unwrap_or(b'?'));
                at += 3;
                continue;
            }
        }
        bytes.push(raw[at]);
        at += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Default)]
struct FeedEntry {
    title: String,
    author: String,
    updated: String,
    href: String,
}

pub fn parse_feed(body: &str) -> Vec<FeedWork> {
    let mut decoded = Vec::new();
    let mut raw_entries = Vec::new();
    let mut entry: Option<FeedEntry> = None;
    let mut field = "";
    let mut owned_texts: Vec<(usize, usize, &'static str)> = Vec::new();
    scan(body, &mut decoded, |event| match event {
        Event::Open { name, .. } => {
            let (_, local) = split_name(name);
            if local == "entry" && raw_entries.len() < MAX_FEED_WORKS {
                entry = Some(FeedEntry::default());
            } else if entry.is_some() && matches!(local, "title" | "name" | "updated") {
                field = match local {
                    "title" => "title",
                    "name" => "author",
                    _ => "updated",
                };
            } else if local == "link" && entry.is_some() {
                let rel = event.attribute("rel").unwrap_or_default();
                if rel.is_empty() || rel == "alternate" {
                    if let Some(href) = event.attribute("href") {
                        if href.contains("/works/") {
                            if let Some(current) = entry.as_mut() {
                                current.href = href;
                            }
                        }
                    }
                }
            }
        }
        Event::Text(text) => {
            if let Some(current) = entry.as_mut() {
                set_entry_field(current, field, text);
            }
        }
        Event::Owned(index) => {
            if entry.is_some() {
                owned_texts.push((raw_entries.len(), index, field));
            }
        }
        Event::Close { name } => {
            let (_, local) = split_name(name);
            if local == "entry" {
                if let Some(current) = entry.take() {
                    raw_entries.push(current);
                }
                field = "";
            } else if matches!(local, "title" | "name" | "updated") {
                field = "";
            }
        }
    });
    for (entry_index, text_index, owned_field) in owned_texts {
        if let (Some(entry), Some(text)) =
            (raw_entries.get_mut(entry_index), decoded.get(text_index))
        {
            set_entry_field(entry, owned_field, text);
        }
    }
    raw_entries
        .into_iter()
        .filter_map(|mut entry| {
            if entry.author.is_empty() {
                if let Some((title, author)) = entry
                    .title
                    .rsplit_once(" by ")
                    .map(|(title, author)| (title.to_owned(), author.to_owned()))
                {
                    entry.title = title;
                    entry.author = author;
                }
            }
            Some(FeedWork {
                id: work_id(&entry.href)?,
                title: bounded(&plain(&entry.title, TITLE_MAX), TITLE_MAX),
                author: bounded(&plain(&entry.author, AUTHOR_MAX), AUTHOR_MAX),
                updated: bounded(&entry.updated, DATE_MAX),
            })
        })
        .take(MAX_FEED_WORKS)
        .collect()
}

fn set_entry_field(entry: &mut FeedEntry, field: &str, text: &str) {
    let target = match field {
        "title" => &mut entry.title,
        "author" => &mut entry.author,
        "updated" => &mut entry.updated,
        _ => return,
    };
    if target.len() < SUMMARY_MAX {
        target.push_str(text);
    }
}

pub fn work_id(text: &str) -> Option<String> {
    let part = text
        .trim()
        .trim_end_matches('/')
        .split("/works/")
        .nth(1)
        .unwrap_or(text.trim())
        .split(['/', '?', '#'])
        .next()?;
    (!part.is_empty() && part.len() <= 20 && part.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| part.to_owned())
}

pub fn encode_works(works: &[Work]) -> Vec<u8> {
    let mut lines = vec!["v2".to_owned()];
    lines.extend(works.iter().take(MAX_WORKS).map(|work| {
        [
            work.id.clone(),
            escape(&work.title),
            escape(&work.author),
            escape(&work.fandom),
            escape(&work.rating),
            escape(&work.warnings),
            escape(&work.summary),
            work.chapters.to_string(),
            work.total_chapters
                .map_or_else(|| "-".to_owned(), |total| total.to_string()),
            u8::from(work.complete).to_string(),
            escape(&work.updated),
            escape(&work.epub),
            match work.download {
                DownloadState::NotDownloaded => "n",
                DownloadState::Downloaded => "d",
                DownloadState::UpdateAvailable => "u",
                DownloadState::Removed => "r",
            }
            .to_owned(),
            u8::from(work.adult).to_string(),
        ]
        .join("\t")
    }));
    lines.join("\n").into_bytes()
}

pub fn decode_works(bytes: &[u8]) -> Vec<Work> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    if lines.next() != Some("v2") {
        return decode_legacy(text);
    }
    lines
        .filter_map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 14 {
                return None;
            }
            let id = fields[0];
            if work_id(id).as_deref() != Some(id) {
                return None;
            }
            Some(Work {
                id: id.to_owned(),
                title: unescape(fields[1]),
                author: unescape(fields[2]),
                fandom: unescape(fields[3]),
                rating: unescape(fields[4]),
                warnings: unescape(fields[5]),
                summary: unescape(fields[6]),
                chapters: fields[7].parse().ok()?,
                total_chapters: (fields[8] != "-").then(|| fields[8].parse().ok()).flatten(),
                complete: fields[9] == "1",
                updated: unescape(fields[10]),
                epub: unescape(fields[11]),
                download: match fields[12] {
                    "d" => DownloadState::Downloaded,
                    "u" => DownloadState::UpdateAvailable,
                    "r" => DownloadState::Removed,
                    _ => DownloadState::NotDownloaded,
                },
                adult: fields[13] == "1",
            })
        })
        .take(MAX_WORKS)
        .collect()
}

fn decode_legacy(text: &str) -> Vec<Work> {
    text.lines()
        .filter_map(|line| {
            let pipe = line.split('|').collect::<Vec<_>>();
            if pipe.len() == 3 && work_id(pipe[0]).is_some() {
                return Some(Work {
                    id: pipe[0].to_owned(),
                    title: pipe[1].to_owned(),
                    epub: pipe[2].to_owned(),
                    download: DownloadState::Downloaded,
                    ..Work::default()
                });
            }
            let tab = line.split('\t').collect::<Vec<_>>();
            (tab.len() == 4 && work_id(tab[0]).is_some()).then(|| Work {
                id: tab[0].to_owned(),
                title: tab[1].to_owned(),
                author: tab[2].to_owned(),
                epub: tab[3].to_owned(),
                download: DownloadState::Downloaded,
                ..Work::default()
            })
        })
        .take(MAX_WORKS)
        .collect()
}

pub fn encode_tags(tags: &[FollowedTag]) -> Vec<u8> {
    tags.iter()
        .take(MAX_TAGS)
        .map(|tag| format!("{}\t{}", escape(&tag.name), tag.slug))
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

pub fn decode_tags(bytes: &[u8]) -> Vec<FollowedTag> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (name, slug) = line.split_once('\t')?;
            (!slug.is_empty() && slug.len() <= 512).then(|| FollowedTag {
                name: bounded(&unescape(name), 160),
                slug: slug.to_owned(),
            })
        })
        .take(MAX_TAGS)
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\t', "%09")
        .replace('\n', "%0A")
        .replace('\r', "%0D")
}

fn unescape(text: &str) -> String {
    text.replace("%0D", "\r")
        .replace("%0A", "\n")
        .replace("%09", "\t")
        .replace("%25", "%")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: &str = r#"
      <html><head><title>Fallback | Archive of Our Own</title></head><body>
      <h2 class="title heading">The Lantern Library</h2>
      <h3 class="byline heading"><a rel="author">River Quill</a></h3>
      <dd class="rating tags"><ul><li><a>Teen And Up Audiences</a></li></ul></dd>
      <dd class="warning tags"><ul><li><a>No Archive Warnings Apply</a></li></ul></dd>
      <dd class="fandom tags"><ul><li><a>Public Domain Fairy Tales</a></li></ul></dd>
      <blockquote class="userstuff summary module"><p>A synthetic fixture.</p></blockquote>
      <dd class="updated">2026-09-01</dd><dd class="chapters">12/?</dd>
      <a href="/downloads/4242/The_Lantern_Library.epub?updated_at=1">EPUB</a>
      </body></html>
    "#;

    #[test]
    fn parses_complete_metadata_and_epub() {
        let ParsedWork::Work(work) = parse_work_page("4242", WORK) else {
            panic!("work was not parsed");
        };
        assert_eq!(work.title, "The Lantern Library");
        assert_eq!(work.author, "River Quill");
        assert_eq!(work.fandom, "Public Domain Fairy Tales");
        assert_eq!(work.rating, "Teen And Up Audiences");
        assert_eq!(work.warnings, "No Archive Warnings Apply");
        assert_eq!(work.chapters_label(), "12/? WIP");
        assert!(work.epub.ends_with(".epub?updated_at=1"));
    }

    #[test]
    fn adult_interstitial_is_not_mistaken_for_metadata() {
        assert_eq!(
            parse_work_page(
                "42",
                r#"<p>This work could have adult content.</p><a href="/works/42?view_adult=true">Proceed</a>"#
            ),
            ParsedWork::AdultInterstitial
        );
    }

    #[test]
    fn atom_feed_is_bounded_and_uses_structured_links() {
        let atom = r#"
          <feed xmlns="http://www.w3.org/2005/Atom">
            <entry><title>The Clockwork Garden</title><author><name>North Star</name></author>
            <updated>2026-09-01T00:00:00Z</updated>
            <link rel="alternate" href="https://archiveofourown.org/works/9001"/></entry>
          </feed>
        "#;
        assert_eq!(
            parse_feed(atom),
            [FeedWork {
                id: "9001".into(),
                title: "The Clockwork Garden".into(),
                author: "North Star".into(),
                updated: "2026-09-01T00:00:00Z".into(),
            }]
        );
    }

    #[test]
    fn durable_catalogue_round_trips_all_metadata() {
        let ParsedWork::Work(work) = parse_work_page("4242", WORK) else {
            panic!("work was not parsed");
        };
        let mut work = *work;
        work.download = DownloadState::UpdateAvailable;
        assert_eq!(decode_works(&encode_works(&[work.clone()])), [work]);
        let tag = parse_tag("Public Domain Fairy Tales").unwrap();
        assert_eq!(decode_tags(&encode_tags(std::slice::from_ref(&tag))), [tag]);
    }

    #[test]
    fn the_original_pipe_shelf_migrates() {
        let works = decode_works(b"123|A saved work|work-123.epub\n");
        assert_eq!(works.len(), 1);
        assert_eq!(works[0].id, "123");
        assert_eq!(works[0].title, "A saved work");
        assert_eq!(works[0].epub, "work-123.epub");
        assert_eq!(works[0].download, DownloadState::Downloaded);
    }
}
