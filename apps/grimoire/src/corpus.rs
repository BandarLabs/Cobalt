//! Generated, checked-in SRD corpus reader. See `tools/build_corpus.py`.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub edition: u16,
    pub kind: String,
    pub name: String,
    pub subtitle: String,
    pub body: String,
    pub tags: String,
}

const RAW: &str = include_str!("../data/corpus.tsv");

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            match character {
                'n' => out.push('\n'),
                '\\' => out.push('\\'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            out.push(character);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

#[must_use]
pub fn load() -> Vec<Entry> {
    RAW.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(Entry {
                edition: fields.next()?.parse().ok()?,
                kind: fields.next()?.to_owned(),
                name: unescape(fields.next()?),
                subtitle: unescape(fields.next()?),
                body: unescape(fields.next()?),
                tags: unescape(fields.next().unwrap_or_default()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::load;

    #[test]
    fn generated_corpus_is_sorted_complete_and_bounded() {
        let corpus = load();
        assert_eq!(corpus.len(), 1_349);
        assert!(include_str!("../data/corpus.tsv").len() <= 6 * 1024 * 1024);
        assert!(corpus.windows(2).all(|pair| {
            (&pair[0].kind, pair[0].edition, pair[0].name.to_lowercase())
                <= (&pair[1].kind, pair[1].edition, pair[1].name.to_lowercase())
        }));
        for kind in ["spell", "monster", "condition", "rule", "item"] {
            assert!(corpus.iter().any(|entry| entry.kind == kind), "{kind}");
        }
    }

    /// Bodies are the reader's markup, written once. Escaping them twice put a
    /// visible backslash-n between every paragraph, and shipping the snapshots'
    /// Markdown as written put "## Traps" and rows of pipes on the panel.
    #[test]
    fn bodies_are_readable_markup_rather_than_escapes_or_markdown() {
        let corpus = load();
        assert!(
            !corpus.iter().any(|entry| entry.body.contains("\\n")),
            "an entry body still carries an escape sequence"
        );
        assert!(
            !corpus
                .iter()
                .any(|entry| entry.body.contains("**") || entry.body.contains("## ")),
            "an entry body still carries Markdown"
        );
        let rule = corpus
            .iter()
            .find(|entry| entry.kind == "rule" && entry.body.len() > 4000)
            .expect("a long rule section");
        assert!(rule.body.contains("<h"), "a long rule lost its headings");
        // A table arrives as one paragraph per row, labelled by its headings:
        // columns cannot hold their text at the larger interface sizes.
        assert!(
            !corpus.iter().any(|entry| entry.body.contains("<table>")),
            "a body still carries a table the panel cannot draw at every size"
        );
        assert!(
            corpus.iter().any(|entry| entry.kind == "rule"
                && entry.body.contains("<p><strong>Setback</strong> Save DC: ")),
            "the trap table lost its rows"
        );
        assert!(
            corpus
                .iter()
                .all(|entry| entry.kind != "monster" || entry.body.starts_with("<p>AC ")),
            "a stat block lost its opening line"
        );
    }

    #[test]
    fn prefix_search_handles_names_case_insensitively() {
        let corpus = load();
        assert!(corpus
            .iter()
            .any(|entry| entry.kind == "spell" && entry.name.to_lowercase().starts_with("fire")));
        assert!(
            corpus
                .iter()
                .any(|entry| entry.kind == "monster"
                    && entry.name.to_lowercase().starts_with("adult"))
        );
    }
}
