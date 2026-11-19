use crate::md::render;

/// Note boundary inside the legacy packed index. Markdown thematic breaks are
/// a single `---`, so a vault that uses those must not be split on the same
/// bytes.
pub const INDEX_SEPARATOR: &str = "\n\n---vault-note---\n\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Note {
    pub path: String,
    pub body: String,
}

impl Note {
    pub fn title(&self) -> String {
        self.path
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .trim_end_matches(".md")
            .replace('-', " ")
    }
    #[allow(dead_code)]
    pub fn rendered(&self) -> String {
        render(&self.body)
    }
    pub fn tags(&self) -> Vec<String> {
        self.body
            .split_whitespace()
            .filter_map(|word| word.strip_prefix('#'))
            .map(|tag| {
                tag.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '-')
                    .to_owned()
            })
            .filter(|tag| !tag.is_empty())
            .collect()
    }
    pub fn links(&self) -> Vec<String> {
        let mut links = markdown_links(&self.body);
        links.extend(wiki_links(&self.body));
        links
    }
}

#[allow(dead_code)]
pub fn encode_index(notes: &[Note]) -> String {
    notes
        .iter()
        .map(|note| format!("{}\n{}", note.path, note.body))
        .collect::<Vec<_>>()
        .join(INDEX_SEPARATOR)
}

pub fn decode_index(raw: &str) -> Vec<Note> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    raw.split(INDEX_SEPARATOR)
        .filter_map(|part| {
            let part = part.trim_start_matches('\n');
            part.split_once('\n').map(|(path, body)| Note {
                path: path.to_owned(),
                body: body.to_owned(),
            })
        })
        .filter(|note| !note.path.is_empty())
        .collect()
}

fn markdown_links(body: &str) -> Vec<String> {
    body.match_indices("](")
        .filter_map(|(start, _)| body[start + 2..].split(')').next())
        .filter(|link| link.to_ascii_lowercase().ends_with(".md"))
        .map(str::to_owned)
        .collect()
}

fn wiki_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(open) = body[from..].find("[[") {
        let rest = &body[from + open + 2..];
        let Some(close) = rest.find("]]") else {
            break;
        };
        let inner = &rest[..close];
        from += open + 2 + close + 2;
        if inner.starts_with('{') {
            continue;
        }
        let target = inner
            .split('|')
            .next()
            .unwrap_or(inner)
            .split('#')
            .next()
            .unwrap_or(inner)
            .trim();
        if target.is_empty() || looks_like_attachment(target) {
            continue;
        }
        links.push(target.to_owned());
    }
    links
}

fn looks_like_attachment(target: &str) -> bool {
    PathExt(target).has_non_note_extension()
}

struct PathExt<'a>(&'a str);

impl PathExt<'_> {
    fn has_non_note_extension(self) -> bool {
        let Some((_, ext)) = self.0.rsplit_once('.') else {
            return false;
        };
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf" | "mp3" | "mp4" | "canvas" | "svg"
        )
    }
}

pub fn link_matches_path(link: &str, path: &str) -> bool {
    let link = link.trim();
    if link.is_empty() {
        return false;
    }
    if link == path {
        return true;
    }
    let with_md = if link.to_ascii_lowercase().ends_with(".md") {
        link.to_owned()
    } else {
        format!("{link}.md")
    };
    if with_md == path {
        return true;
    }
    let path_stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md");
    let link_stem = with_md
        .rsplit('/')
        .next()
        .unwrap_or(&with_md)
        .trim_end_matches(".md");
    path_stem.eq_ignore_ascii_case(link_stem)
}

pub fn search(notes: &[Note], query: &str) -> Vec<(usize, String)> {
    let query = query.to_lowercase();
    notes
        .iter()
        .enumerate()
        .filter_map(|(index, note)| {
            note.body.lines().find_map(|line| {
                let at = line.to_lowercase().find(&query)?;
                Some((index, excerpt(line, at)))
            })
        })
        .take(200)
        .collect()
}

/// The most characters of a matching line shown under a result.
const EXCERPT_CHARS: usize = 120;
/// How many of them come before the match, so it sits near the start.
const EXCERPT_LEAD: usize = 40;

/// The part of `line` around the match that starts at byte `at` of its
/// lowercase form.
///
/// A whole paragraph under every result filled the panel with one hit and
/// left the reader hunting for the word inside it. A window that opens just
/// before the match, cut at spaces and marked where it was cut, shows the
/// words around it and leaves room for the other notes.
fn excerpt(line: &str, at: usize) -> String {
    let line = line.trim();
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= EXCERPT_CHARS {
        return line.to_owned();
    }
    // Lowercasing can change byte lengths, so the match is found again by
    // character position, counted on the lowercase text it was found in.
    let lead = line
        .to_lowercase()
        .get(..at)
        .map_or(0, |before| before.chars().count())
        .min(chars.len());
    let mut start = lead.saturating_sub(EXCERPT_LEAD);
    if start > 0 {
        start = (start..lead)
            .find(|&index| chars[index - 1] == ' ')
            .unwrap_or(start);
    }
    let mut end = (start + EXCERPT_CHARS).min(chars.len());
    if end < chars.len() {
        end = (start + 1..end)
            .rev()
            .find(|&index| chars[index] == ' ')
            .unwrap_or(end);
    }
    let middle: String = chars[start..end].iter().collect();
    let middle = middle.trim();
    match (start > 0, end < chars.len()) {
        (true, true) => format!("…{middle}…"),
        (true, false) => format!("…{middle}"),
        (false, true) => format!("{middle}…"),
        (false, false) => middle.to_owned(),
    }
}

#[allow(dead_code)]
pub fn backlinks(notes: &[Note], path: &str) -> Vec<(usize, String)> {
    notes
        .iter()
        .enumerate()
        .filter_map(|(index, note)| {
            let matched = note
                .links()
                .into_iter()
                .find(|link| link_matches_path(link, path))?;
            let line = note
                .body
                .lines()
                .find(|line| line.contains(&matched) || line.contains(path))
                .unwrap_or("")
                .trim()
                .to_owned();
            Some((index, line))
        })
        .collect()
}
