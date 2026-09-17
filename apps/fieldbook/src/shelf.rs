//! Decode-only copy of the Fieldbook shelf manifest, kept in step with the
//! host-side codec in the `kobo fieldbook` companion. The CLI owns writing;
//! the app only reads, so a writer here would be dead code.

use kobo_json::Value;

pub const MANIFEST: &str = "packs.v1";
pub const MAX_MANIFEST: usize = 512 * 1024;
const FORMAT: &str = "fieldbook-shelf";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Species {
    pub code: String,
    pub common: String,
    pub scientific: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub region: String,
    pub issued: String,
    pub species: Vec<Species>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportFailure {
    pub input: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Shelf {
    pub packs: Vec<Pack>,
    pub failures: Vec<ImportFailure>,
}

impl Shelf {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MANIFEST {
            return Err("the Fieldbook shelf manifest is too large".to_owned());
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| "the shelf manifest is not UTF-8".to_owned())?;
        let value = kobo_json::parse(text).map_err(|error| format!("shelf manifest: {error}"))?;
        let root = &value;
        if root.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err("the shelf manifest is not a Fieldbook shelf".to_owned());
        }
        if root.get("version").and_then(Value::as_str) != Some("1") {
            return Err("the shelf manifest version is not supported".to_owned());
        }
        let mut packs = Vec::new();
        for entry in root
            .get("packs")
            .and_then(Value::as_array)
            .ok_or_else(|| "the shelf manifest has no pack list".to_owned())?
        {
            let text = |key: &str| -> Result<String, String> {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("a shelf pack has no {key}"))
            };
            let mut species = Vec::new();
            for bird in entry
                .get("species")
                .and_then(Value::as_array)
                .ok_or_else(|| "a shelf pack has no species list".to_owned())?
            {
                let bird_text = |key: &str| -> Result<String, String> {
                    bird.get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| format!("a shelf species has no {key}"))
                };
                species.push(Species {
                    code: bird_text("code")?,
                    common: bird_text("common")?,
                    scientific: bird_text("scientific")?,
                });
            }
            if species.is_empty() {
                return Err("a shelf pack has an empty species list".to_owned());
            }
            packs.push(Pack {
                id: text("id")?,
                title: text("title")?,
                region: text("region")?,
                issued: text("issued")?,
                species,
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
        Ok(Self { packs, failures })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_real_shelf_manifest() {
        let json = r#"{"format":"fieldbook-shelf","version":"1",
            "packs":[{"id":"us-ca-sf","title":"San Francisco Bay","region":"US-CA-SF",
            "issued":"2026-09-01",
            "species":[{"code":"AMRO","common":"American Robin",
            "scientific":"Turdus migratorius"}]}],
            "failures":[{"input":"old-pack.json","reason":"unknown format"}]}"#;
        let shelf = Shelf::decode(json.as_bytes()).expect("manifest");
        assert_eq!(shelf.packs.len(), 1);
        assert_eq!(shelf.packs[0].species[0].code, "AMRO");
        assert_eq!(shelf.failures[0].input, "old-pack.json");
    }

    #[test]
    fn rejects_another_shelf_format() {
        assert!(Shelf::decode(br#"{"format":"other","version":"1","packs":[]}"#).is_err());
    }

    #[test]
    fn rejects_a_pack_without_species() {
        let json = r#"{"format":"fieldbook-shelf","version":"1",
            "packs":[{"id":"p","title":"t","region":"r","issued":"d","species":[]}]}"#;
        assert!(Shelf::decode(json.as_bytes()).is_err());
    }
}
