#![forbid(unsafe_code)]

//! Offline dictionaries owned by the platform.
//!
//! Dictionaries are UTF-8 TSV files. Comment metadata may precede entries:
//! `# name=Oxford`, `# language=en`, `# priority=10`. Every remaining line is
//! `headword<TAB>definition`. A malformed line costs that line, never the
//! other dictionaries or the reading session.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub const MAX_DICTIONARIES: usize = 32;
pub const MAX_DICTIONARY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ENTRIES_PER_LOOKUP: usize = 8;
pub const MAX_HEADWORD_BYTES: usize = 128;
pub const MAX_DEFINITION_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub dictionary: String,
    pub language: String,
    pub headword: String,
    pub definition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StoredEntry {
    headword: String,
    definition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dictionary {
    pub name: String,
    pub language: String,
    pub priority: i16,
    entries: BTreeMap<String, Vec<StoredEntry>>,
    pub rejected_lines: usize,
}

impl Dictionary {
    #[must_use]
    pub fn from_tsv(fallback_name: &str, source: &str) -> Self {
        let mut dictionary = Self {
            name: bounded_label(fallback_name, "Dictionary"),
            language: "und".to_owned(),
            priority: 0,
            entries: BTreeMap::new(),
            rejected_lines: 0,
        };
        for line in source.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(metadata) = line.strip_prefix('#') {
                dictionary.apply_metadata(metadata.trim());
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            let Some((headword, definition)) = line.split_once('\t') else {
                dictionary.rejected_lines += 1;
                continue;
            };
            let headword = headword.trim();
            let definition = definition.trim();
            if headword.is_empty()
                || definition.is_empty()
                || headword.len() > MAX_HEADWORD_BYTES
                || definition.len() > MAX_DEFINITION_BYTES
            {
                dictionary.rejected_lines += 1;
                continue;
            }
            let key = normalize(headword);
            if key.is_empty() {
                dictionary.rejected_lines += 1;
                continue;
            }
            dictionary
                .entries
                .entry(key)
                .or_default()
                .push(StoredEntry {
                    headword: headword.to_owned(),
                    definition: definition.to_owned(),
                });
        }
        dictionary
    }

    fn apply_metadata(&mut self, metadata: &str) {
        let Some((key, value)) = metadata.split_once('=') else {
            return;
        };
        match key.trim() {
            "name" => self.name = bounded_label(value.trim(), &self.name),
            "language" if valid_language(value.trim()) => {
                value.trim().clone_into(&mut self.language);
            }
            "priority" => {
                if let Ok(priority) = value.trim().parse() {
                    self.priority = priority;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Index {
    dictionaries: Vec<Dictionary>,
}

impl Index {
    pub fn install(&mut self, dictionary: Dictionary) -> bool {
        if self.dictionaries.len() >= MAX_DICTIONARIES || dictionary.entries.is_empty() {
            return false;
        }
        self.dictionaries.push(dictionary);
        self.dictionaries.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.language.cmp(&right.language))
        });
        true
    }

    #[must_use]
    pub fn load_directory(path: &Path) -> Self {
        let mut index = Self::default();
        let Ok(entries) = fs::read_dir(path) else {
            return index;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "tsv"))
            .collect::<Vec<_>>();
        paths.sort();
        let mut total = 0usize;
        for path in paths.into_iter().take(MAX_DICTIONARIES) {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            let Ok(length) = usize::try_from(metadata.len()) else {
                continue;
            };
            if length > MAX_DICTIONARY_BYTES || total.saturating_add(length) > MAX_TOTAL_BYTES {
                continue;
            }
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            total += length;
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Dictionary");
            let _ = index.install(Dictionary::from_tsv(name, &source));
        }
        index
    }

    #[must_use]
    pub fn lookup(&self, word: &str, language: Option<&str>) -> Vec<Entry> {
        let keys = lookup_keys(word);
        let mut found = Vec::new();
        for dictionary in &self.dictionaries {
            if language.is_some_and(|wanted| {
                dictionary.language != "und"
                    && !dictionary.language.eq_ignore_ascii_case(wanted)
                    && !dictionary.language.starts_with(&format!("{wanted}-"))
            }) {
                continue;
            }
            for key in &keys {
                let Some(entries) = dictionary.entries.get(key) else {
                    continue;
                };
                for entry in entries {
                    found.push(Entry {
                        dictionary: dictionary.name.clone(),
                        language: dictionary.language.clone(),
                        headword: entry.headword.clone(),
                        definition: entry.definition.clone(),
                    });
                    if found.len() >= MAX_ENTRIES_PER_LOOKUP {
                        return found;
                    }
                }
                break;
            }
        }
        found
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.dictionaries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dictionaries.is_empty()
    }
}

#[must_use]
pub fn normalize(value: &str) -> String {
    value
        .trim_matches(|character: char| !character.is_alphanumeric())
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect()
}

fn lookup_keys(word: &str) -> Vec<String> {
    let exact = normalize(word);
    let mut keys = vec![exact.clone()];
    let mut add = |candidate: String| {
        if !candidate.is_empty() && !keys.contains(&candidate) {
            keys.push(candidate);
        }
    };
    if let Some(stem) = exact.strip_suffix("'s") {
        add(stem.to_owned());
    }
    if let Some(stem) = exact.strip_suffix("ies") {
        add(format!("{stem}y"));
    }
    for suffix in ["ing", "ed", "es", "s"] {
        if let Some(stem) = exact
            .strip_suffix(suffix)
            .filter(|stem| stem.chars().count() >= 3)
        {
            add(stem.to_owned());
            if matches!(suffix, "ing" | "ed") {
                add(format!("{stem}e"));
            }
        }
    }
    keys
}

fn bounded_label(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.len() > 96 {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn valid_language(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_normalized_and_inflected_words_are_found_without_a_network() {
        let source = "# name=Pocket English\n# language=en\nCafé\tA small coffee house.\nstory\tAn account of events.\nmake\tTo create.\n";
        let mut index = Index::default();
        assert!(index.install(Dictionary::from_tsv("fallback", source)));
        assert_eq!(index.lookup("CAFÉ!", Some("en"))[0].headword, "Café");
        assert_eq!(index.lookup("stories", Some("en"))[0].headword, "story");
        assert_eq!(index.lookup("making", Some("en"))[0].headword, "make");
    }

    #[test]
    fn dictionary_order_is_priority_then_name_and_results_are_bounded() {
        let mut index = Index::default();
        for number in 0..12 {
            let source =
                format!("# name=D{number:02}\n# priority={number}\nword\tDefinition {number}\n");
            assert!(index.install(Dictionary::from_tsv("fallback", &source)));
        }
        let found = index.lookup("word", None);
        assert_eq!(found.len(), MAX_ENTRIES_PER_LOOKUP);
        assert_eq!(found[0].dictionary, "D11");
    }

    #[test]
    fn malformed_and_oversized_lines_are_isolated() {
        let long = "x".repeat(MAX_DEFINITION_BYTES + 1);
        let dictionary =
            Dictionary::from_tsv("Safe", &format!("broken\ngood\tUseful.\nlarge\t{long}\n"));
        assert_eq!(dictionary.rejected_lines, 2);
        let mut index = Index::default();
        assert!(index.install(dictionary));
        assert_eq!(index.lookup("good", None)[0].definition, "Useful.");
    }
}
