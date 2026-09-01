use crate::md::render;
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
        self.body
            .match_indices("](")
            .filter_map(|(start, _)| self.body[start + 2..].split(')').next())
            .filter(|link| link.to_ascii_lowercase().ends_with(".md"))
            .map(str::to_owned)
            .collect()
    }
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
            if note.links().iter().any(|link| link == path) {
                Some((
                    index,
                    note.body
                        .lines()
                        .find(|line| line.contains(path))
                        .unwrap_or("")
                        .trim()
                        .to_owned(),
                ))
            } else {
                None
            }
        })
        .collect()
}
