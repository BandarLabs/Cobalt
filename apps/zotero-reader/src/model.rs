use kobo_json::{ObjectBuilder, Value};

pub const MAX_ITEMS: usize = 500;
const MAX_COLLECTIONS: usize = 100;
const MAX_TAGS: usize = 10;
const MAX_AUTHORS: usize = 64;
const TEXT_BLOCK_CHARS: usize = 4_096;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Collection {
    pub key: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Paper {
    pub key: String,
    pub version: u32,
    pub title: String,
    pub creators: String,
    pub year: String,
    pub date_added: String,
    pub tags: Vec<String>,
    pub has_pdf: bool,
}

impl Paper {
    pub fn searchable(&self, phrase: &str) -> bool {
        let phrase = phrase.to_lowercase();
        self.title.to_lowercase().contains(&phrase)
            || self.creators.to_lowercase().contains(&phrase)
            || self
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&phrase))
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.creators.is_empty() {
            parts.push(self.creators.clone());
        }
        if !self.year.is_empty() {
            parts.push(self.year.clone());
        }
        parts.join(" · ")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub collection_key: String,
    pub revision: String,
    pub total: usize,
    pub truncated: bool,
    pub papers: Vec<Paper>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Detail {
    pub paper: Paper,
    pub authors: Vec<String>,
    pub abstract_text: String,
    pub venue: String,
    pub doi: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Attachment {
    pub key: String,
    pub version: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Conversion {
    pub state: String,
    pub document_version: Option<String>,
    pub truncated: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FullText {
    pub html: Vec<u8>,
    pub truncated: bool,
}

/// Parses Zotero API v3 collection objects.
pub fn zotero_collections(bytes: &[u8]) -> Option<Vec<Collection>> {
    let root = parse(bytes)?;
    let rows = root.as_array()?;
    let mut parsed = Vec::new();
    for row in rows.iter().take(MAX_COLLECTIONS) {
        let data = row.get("data")?;
        let key = text(data.get("key").or_else(|| row.get("key")), 32);
        let name = text(data.get("name"), 160);
        if valid_key(&key) && !name.is_empty() {
            parsed.push(Collection { key, name });
        }
    }
    Some(parsed)
}

/// Parses one page of top-level Zotero API v3 item objects.
pub fn zotero_items(bytes: &[u8]) -> Option<Vec<Paper>> {
    let root = parse(bytes)?;
    let rows = root.as_array()?;
    Some(rows.iter().filter_map(zotero_paper).collect())
}

/// Parses a single Zotero API v3 bibliographic item.
pub fn zotero_detail(bytes: &[u8]) -> Option<Detail> {
    let root = parse(bytes)?;
    let paper = zotero_paper(&root)?;
    let data = root.get("data")?;
    let authors = creator_names(data, MAX_AUTHORS);
    let venue = [
        "publicationTitle",
        "bookTitle",
        "proceedingsTitle",
        "conferenceName",
        "websiteTitle",
        "publisher",
    ]
    .iter()
    .find_map(|field| {
        let value = text(data.get(field), 512);
        (!value.is_empty()).then_some(value)
    })
    .unwrap_or_default();
    Some(Detail {
        paper,
        authors,
        abstract_text: text(data.get("abstractNote"), 96_000),
        venue,
        doi: text(data.get("DOI"), 512),
        url: text(data.get("url"), 2_048),
    })
}

/// Selects the first stored PDF attachment from an item's children.
pub fn zotero_pdf_attachment(bytes: &[u8]) -> Result<Option<Attachment>, ()> {
    let root = parse(bytes).ok_or(())?;
    let rows = root.as_array().ok_or(())?;
    for row in rows {
        let Some(data) = row.get("data") else {
            continue;
        };
        if data.get("itemType").and_then(Value::as_str) != Some("attachment")
            || data.get("contentType").and_then(Value::as_str) != Some("application/pdf")
        {
            continue;
        }
        let link_mode = data
            .get("linkMode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(link_mode, "imported_file" | "imported_url") {
            continue;
        }
        let key = text(data.get("key").or_else(|| row.get("key")), 32);
        if !valid_key(&key) {
            continue;
        }
        return Ok(Some(Attachment {
            key,
            version: u32::try_from(number(data.get("version").or_else(|| row.get("version"))))
                .unwrap_or(u32::MAX),
        }));
    }
    Ok(None)
}

/// Turns Zotero's indexed plain text into bounded, inert HTML for `BookView`.
pub fn zotero_fulltext(bytes: &[u8], title: &str, maximum: usize) -> Option<FullText> {
    let root = parse(bytes)?;
    let content = root.get("content")?.as_str()?;
    let incomplete = count_is_incomplete(&root, "indexedPages", "totalPages")
        || count_is_incomplete(&root, "indexedChars", "totalChars");
    let notice = "<p><strong>Text truncated. The Zotero index or this reader did not contain the complete document.</strong></p>";
    let closing = "</body></html>";
    if maximum < notice.len() + closing.len() + 32 {
        return None;
    }

    let mut html = String::from("<html><body>");
    if !title.is_empty() {
        let heading = format!("<h1>{}</h1>", escape_html(title));
        if html.len() + heading.len() + notice.len() + closing.len() <= maximum {
            html.push_str(&heading);
        }
    }

    let mut truncated = incomplete;
    for paragraph in plain_text_blocks(content) {
        let block = format!("<p>{}</p>", escape_html(&paragraph));
        if html.len() + block.len() + notice.len() + closing.len() > maximum {
            truncated = true;
            break;
        }
        html.push_str(&block);
    }
    if truncated {
        html.push_str(notice);
    }
    html.push_str(closing);
    Some(FullText {
        html: html.into_bytes(),
        truncated,
    })
}

/// Encodes the normalized snapshot stored on the reader, not an upstream body.
pub fn encode_snapshot(snapshot: &Snapshot) -> Vec<u8> {
    let items: Vec<Value> = snapshot
        .papers
        .iter()
        .map(|paper| {
            ObjectBuilder::new()
                .set("key", paper.key.clone())
                .set("version", paper.version)
                .set("title", paper.title.clone())
                .set("creator_summary", paper.creators.clone())
                .set("year", paper.year.clone())
                .set("date_added", paper.date_added.clone())
                .set("tags", paper.tags.clone())
                .set("has_stored_pdf", paper.has_pdf)
                .build()
        })
        .collect();
    ObjectBuilder::new()
        .set("collection_key", snapshot.collection_key.clone())
        .set("revision", snapshot.revision.clone())
        .set("total", u32::try_from(snapshot.total).unwrap_or(u32::MAX))
        .set("truncated", snapshot.truncated)
        .set("items", items)
        .build()
        .to_json()
        .into_bytes()
}

/// Parses a normalized snapshot previously stored by [`encode_snapshot`].
pub fn snapshot(bytes: &[u8]) -> Option<Snapshot> {
    let root = parse(bytes)?;
    let rows = root.get("items")?.as_array()?;
    let mut papers = Vec::new();
    for row in rows.iter().take(MAX_ITEMS) {
        if let Some(paper) = normalized_paper(row) {
            papers.push(paper);
        }
    }
    papers.sort_by(|left, right| right.date_added.cmp(&left.date_added));
    Some(Snapshot {
        collection_key: text(root.get("collection_key"), 32),
        revision: text(root.get("revision"), 32),
        total: number(root.get("total")),
        truncated: flag(root.get("truncated")),
        papers,
    })
}

/// Parses conversion state from the optional Zotero conversion bridge.
pub fn conversion(bytes: &[u8]) -> Option<Conversion> {
    let root = parse(bytes)?;
    let state = text(root.get("state"), 32);
    if !matches!(
        state.as_str(),
        "missing_pdf" | "queued" | "running" | "ready" | "failed"
    ) {
        return None;
    }
    Some(Conversion {
        state,
        document_version: root
            .get("document_version")
            .and_then(Value::as_str)
            .map(|value| clean(value, 64)),
        truncated: flag(root.get("truncated")),
        message: root
            .get("message")
            .and_then(Value::as_str)
            .map(|value| clean(value, 512)),
    })
}

fn zotero_paper(value: &Value) -> Option<Paper> {
    let data = value.get("data")?;
    let key = text(data.get("key").or_else(|| value.get("key")), 32);
    let title = text(data.get("title"), 512);
    if !valid_key(&key) || title.is_empty() {
        return None;
    }
    let authors = creator_names(data, MAX_AUTHORS);
    let creators = clean(
        &match authors.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [one, two] => format!("{one}, {two}"),
            [one, ..] => format!("{one} et al."),
        },
        160,
    );
    let publication_date = text(data.get("date"), 128);
    Some(Paper {
        key,
        version: u32::try_from(number(data.get("version").or_else(|| value.get("version"))))
            .unwrap_or(u32::MAX),
        title,
        creators,
        year: year(&publication_date),
        date_added: text(data.get("dateAdded"), 64),
        tags: tags(data),
        has_pdf: false,
    })
}

fn normalized_paper(value: &Value) -> Option<Paper> {
    let key = text(value.get("key"), 32);
    let title = text(value.get("title"), 512);
    if !valid_key(&key) || title.is_empty() {
        return None;
    }
    let tags = value
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(|tag| clean(tag, 48))
                .filter(|tag| !tag.is_empty())
                .take(MAX_TAGS)
                .collect()
        })
        .unwrap_or_default();
    Some(Paper {
        key,
        version: u32::try_from(number(value.get("version"))).unwrap_or(u32::MAX),
        title,
        creators: text(value.get("creator_summary"), 160),
        year: text(value.get("year"), 16),
        date_added: text(value.get("date_added"), 64),
        tags,
        has_pdf: flag(value.get("has_stored_pdf")),
    })
}

fn creator_names(data: &Value, maximum: usize) -> Vec<String> {
    data.get("creators")
        .and_then(Value::as_array)
        .map(|creators| {
            creators
                .iter()
                .filter_map(|creator| {
                    let single = text(creator.get("name"), 128);
                    if !single.is_empty() {
                        return Some(single);
                    }
                    let first = text(creator.get("firstName"), 64);
                    let last = text(creator.get("lastName"), 64);
                    let joined = clean(&format!("{first} {last}"), 128);
                    (!joined.is_empty()).then_some(joined)
                })
                .take(maximum)
                .collect()
        })
        .unwrap_or_default()
}

fn tags(data: &Value) -> Vec<String> {
    data.get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| tag.get("tag").and_then(Value::as_str))
                .map(|tag| clean(tag, 48))
                .filter(|tag| !tag.is_empty())
                .take(MAX_TAGS)
                .collect()
        })
        .unwrap_or_default()
}

fn count_is_incomplete(root: &Value, indexed: &str, total: &str) -> bool {
    let indexed = number(root.get(indexed));
    let total = number(root.get(total));
    total > 0 && indexed < total
}

fn plain_text_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    for paragraph in content.split("\n\n") {
        let compact = paragraph
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if compact.is_empty() {
            continue;
        }
        let mut chunk = String::new();
        let mut count = 0;
        for character in compact.chars() {
            if count >= TEXT_BLOCK_CHARS {
                blocks.push(std::mem::take(&mut chunk));
                count = 0;
            }
            chunk.push(character);
            count += 1;
        }
        if !chunk.is_empty() {
            blocks.push(chunk);
        }
    }
    blocks
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            '\0' => escaped.push(' '),
            character => escaped.push(character),
        }
    }
    escaped
}

fn parse(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    kobo_json::parse(text).ok()
}

fn text(value: Option<&Value>, maximum: usize) -> String {
    value
        .and_then(Value::as_str)
        .map_or_else(String::new, |value| clean(value, maximum))
}

fn clean(value: &str, maximum: usize) -> String {
    value
        .replace(['\0', '\t', '\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(maximum)
        .collect()
}

fn year(value: &str) -> String {
    value
        .as_bytes()
        .windows(4)
        .find(|window| window.iter().all(u8::is_ascii_digit))
        .and_then(|window| std::str::from_utf8(window).ok())
        .unwrap_or_default()
        .to_owned()
}

fn number(value: Option<&Value>) -> usize {
    value
        .and_then(Value::as_i64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

fn flag(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

fn valid_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::{
        encode_snapshot, snapshot, zotero_collections, zotero_detail, zotero_fulltext,
        zotero_items, zotero_pdf_attachment, Paper, Snapshot,
    };

    const COLLECTIONS: &[u8] = include_bytes!("../fixtures/collections.json");
    const ITEMS: &[u8] = include_bytes!("../fixtures/items.json");
    const DETAIL: &[u8] = include_bytes!("../fixtures/item.json");
    const CHILDREN: &[u8] = include_bytes!("../fixtures/children.json");
    const FULLTEXT: &[u8] = include_bytes!("../fixtures/fulltext.json");

    #[test]
    fn zotero_fixtures_map_to_the_reader_contract() {
        let collections = zotero_collections(COLLECTIONS).expect("collections parse");
        assert_eq!(collections[0].name, "Reading Queue");
        let papers = zotero_items(ITEMS).expect("items parse");
        assert_eq!(papers[0].creators, "Ada Lovelace et al.");
        assert_eq!(papers[0].year, "2026");
        assert!(papers[0].searchable("SYSTEMS"));
        let detail = zotero_detail(DETAIL).expect("detail parses");
        assert_eq!(detail.venue, "Journal of Bounded Systems");
        assert_eq!(detail.authors.len(), 3);
        let attachment = zotero_pdf_attachment(CHILDREN)
            .expect("children parse")
            .expect("stored PDF");
        assert_eq!(attachment.key, "PDF12345");
    }

    #[test]
    fn normalized_snapshot_round_trips_and_sorts() {
        let mut papers = zotero_items(ITEMS).expect("items parse");
        papers.reverse();
        let original = Snapshot {
            collection_key: "COLL1234".to_owned(),
            revision: "9".to_owned(),
            total: papers.len(),
            truncated: false,
            papers,
        };
        let decoded = snapshot(&encode_snapshot(&original)).expect("cache parses");
        assert_eq!(decoded.collection_key, original.collection_key);
        assert!(decoded.papers[0].date_added >= decoded.papers[1].date_added);
    }

    #[test]
    fn five_hundred_worst_case_summaries_fit_the_shelf_envelope() {
        let paper = Paper {
            key: "PAPER001".to_owned(),
            version: u32::MAX,
            title: "界".repeat(512),
            creators: "界".repeat(160),
            year: "2026".to_owned(),
            date_added: "2026-08-21T12:00:00Z".to_owned(),
            tags: vec!["界".repeat(48); 10],
            has_pdf: false,
        };
        let snapshot = Snapshot {
            collection_key: "COLL1234".to_owned(),
            revision: "4294967295".to_owned(),
            total: 500,
            truncated: true,
            papers: vec![paper; 500],
        };

        assert!(encode_snapshot(&snapshot).len() <= 4 * 1024 * 1024);
    }

    #[test]
    fn indexed_text_is_inert_and_truncated_at_created_block_boundaries() {
        let fulltext = zotero_fulltext(FULLTEXT, "A <Paper>", 420).expect("full text parses");
        let html = std::str::from_utf8(&fulltext.html).expect("generated UTF-8");
        assert!(html.contains("A &lt;Paper&gt;"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.ends_with("</body></html>"));
        assert!(fulltext.truncated);
        assert!(html.len() <= 420);
    }

    #[test]
    fn linked_pdfs_are_not_treated_as_downloadable_stored_files() {
        let body = br#"[{"key":"LINK1234","data":{"key":"LINK1234","version":1,
          "itemType":"attachment","contentType":"application/pdf","linkMode":"linked_file"}}]"#;
        assert_eq!(zotero_pdf_attachment(body), Ok(None));
    }
}
