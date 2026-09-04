use crate::md::render;

/// Note boundary inside [`INDEX_KEY`]. Markdown thematic breaks are a single
/// `---`, so a vault that uses those must not be split on the same bytes.
pub const INDEX_SEPARATOR: &str = "\n\n---vault-note---\n\n";
pub const INDEX_KEY: &str = "vault-index-v1";

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

fn link_matches_path(link: &str, path: &str) -> bool {
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
            note.body
                .lines()
                .find(|line| line.to_lowercase().contains(&query))
                .map(|line| (index, line.trim().to_owned()))
        })
        .take(200)
        .collect()
}

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
