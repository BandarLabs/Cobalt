//! Decode-only copy of the Music Stand shelf manifest, kept in step with the
//! host-side codec in `crates/kobo-music-host`. The CLI owns writing; the app
//! only reads, so a writer here would be dead code.

use kobo_json::Value;

pub const MANIFEST: &str = "manifest.v1";
pub const MAX_MANIFEST: usize = 256 * 1024;
const FORMAT: &str = "musicstand-shelf";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Score {
    pub id: String,
    pub title: String,
    pub pages: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportFailure {
    pub input: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Manifest {
    pub scores: Vec<Score>,
    pub failures: Vec<ImportFailure>,
}

pub fn page_name(score_id: &str, index: u32) -> String {
    format!("{score_id}-{index:03}.png")
}

impl Manifest {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MANIFEST {
            return Err("the Music Stand shelf manifest is too large".to_owned());
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| "the shelf manifest is not UTF-8".to_owned())?;
        let value = kobo_json::parse(text).map_err(|error| format!("shelf manifest: {error}"))?;
        let root = &value;
        if root.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err("the shelf manifest is not a Music Stand shelf".to_owned());
        }
        if root.get("version").and_then(Value::as_str) != Some("1") {
            return Err("the shelf manifest version is not supported".to_owned());
        }
        let mut scores = Vec::new();
        for entry in root
            .get("scores")
            .and_then(Value::as_array)
            .ok_or_else(|| "the shelf manifest has no score list".to_owned())?
        {
            let text = |key: &str| -> Result<String, String> {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("a shelf score has no {key}"))
            };
            let number = |key: &str| -> Result<u64, String> {
                text(key)?
                    .parse()
                    .map_err(|_| format!("a shelf score has an invalid {key}"))
            };
            scores.push(Score {
                id: text("id")?,
                title: text("title")?,
                pages: u32::try_from(number("pages")?)
                    .map_err(|_| "a shelf score has too many pages".to_owned())?,
                width: u32::try_from(number("width")?)
                    .map_err(|_| "a shelf score page is too wide".to_owned())?,
                height: u32::try_from(number("height")?)
                    .map_err(|_| "a shelf score page is too tall".to_owned())?,
            });
        }
        let mut failures = Vec::new();
        if let Some(entries) = root.get("failures").and_then(Value::as_array) {
            for entry in entries {
                let text = |key: &str| -> Result<String, String> {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| format!("a shelf failure has no {key}"))
                };
                failures.push(ImportFailure {
                    input: text("input")?,
                    reason: text("reason")?,
                });
            }
        }
        Ok(Self { scores, failures })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_real_shelf_manifest() {
        let json = r#"{"format":"musicstand-shelf","version":"1",
            "scores":[{"id":"score-abc","title":"Prelude","digest":"d","pages":"6",
            "width":"1654","height":"2339","added":"1"}],
            "failures":[{"input":"bad.pdf","reason":"no readable pages"}]}"#;
        let manifest = Manifest::decode(json.as_bytes()).expect("manifest");
        assert_eq!(manifest.scores.len(), 1);
        assert_eq!(manifest.scores[0].pages, 6);
        assert_eq!(manifest.failures[0].input, "bad.pdf");
    }

    #[test]
    fn rejects_another_shelf_format() {
        assert!(Manifest::decode(br#"{"format":"other","version":"1","scores":[]}"#).is_err());
    }

    #[test]
    fn page_names_are_zero_padded() {
        assert_eq!(page_name("score-a", 3), "score-a-003.png");
    }
}
