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
    pub photo: Option<SpeciesPhoto>,
}

/// A species photo in a version 2 pack: the pack-relative asset path and
/// the id of its record in attribution.json. On the reader the photo sits
/// on the shelf under the asset's file name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeciesPhoto {
    pub asset: String,
    pub attribution: String,
}

impl SpeciesPhoto {
    /// The flat shelf key the companion publishes the photo under: the
    /// asset's file stem. Shelf keys cap at 64 characters, so the full
    /// digest travels without its extension.
    #[must_use]
    pub fn shelf_key(&self) -> &str {
        let name = self.asset.rsplit('/').next().unwrap_or(&self.asset);
        name.strip_suffix(".jpg").unwrap_or(name)
    }
}

/// One photo's credit from attribution.json.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoCredit {
    pub id: String,
    pub creator: String,
    pub license: String,
}

/// The attribution manifest beside the packs, listing every photo's
/// creator and license.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Attribution {
    pub photos: Vec<PhotoCredit>,
}

impl Attribution {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| "the attribution manifest is not UTF-8".to_owned())?;
        let root =
            kobo_json::parse(text).map_err(|error| format!("attribution manifest: {error}"))?;
        if root.get("format").and_then(Value::as_str) != Some("fieldbook-attribution") {
            return Err("not a Fieldbook attribution manifest".to_owned());
        }
        if root.get("version").and_then(Value::as_str) != Some("1") {
            return Err("the attribution manifest version is not supported".to_owned());
        }
        let mut photos = Vec::new();
        for entry in root
            .get("photos")
            .and_then(Value::as_array)
            .ok_or_else(|| "the attribution manifest has no photo list".to_owned())?
        {
            let text = |key: &str| -> Result<String, String> {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("an attribution record has no {key}"))
            };
            photos.push(PhotoCredit {
                id: text("id")?,
                creator: text("creator")?,
                license: text("license")?,
            });
        }
        Ok(Self { photos })
    }
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
        let version = root.get("version").and_then(Value::as_str);
        if version != Some("1") && version != Some("2") {
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
                let photo = match bird.get("photo") {
                    Some(photo) => Some(SpeciesPhoto {
                        asset: photo
                            .get("asset")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "a species photo has no asset".to_owned())?
                            .to_owned(),
                        attribution: photo
                            .get("attribution")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "a species photo has no attribution".to_owned())?
                            .to_owned(),
                    }),
                    None => None,
                };
                species.push(Species {
                    code: bird_text("code")?,
                    common: bird_text("common")?,
                    scientific: bird_text("scientific")?,
                    photo,
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
    fn decodes_a_version_two_manifest_with_photos() {
        let json = r#"{"format":"fieldbook-shelf","version":"2",
            "packs":[{"id":"us-ca-sf","title":"San Francisco Bay","region":"US-CA-SF",
            "issued":"2026-09-19",
            "species":[{"code":"AMRO","common":"American Robin",
            "scientific":"Turdus migratorius",
            "photo":{"asset":"photos/abc123.jpg","attribution":"abc123"}}]}],
            "failures":[]}"#;
        let shelf = Shelf::decode(json.as_bytes()).expect("v2 manifest");
        let photo = shelf.packs[0].species[0].photo.as_ref().expect("photo");
        assert_eq!(photo.shelf_key(), "abc123");
        assert_eq!(photo.attribution, "abc123");
    }

    #[test]
    fn decodes_the_attribution_manifest() {
        let json = r#"{"format":"fieldbook-attribution","version":"1",
            "photos":[{"id":"abc123","asset":"photos/abc123.jpg","sha256":"abc123",
            "bytes":123,"source":"Avicommons","source_url":"https://avicommons.org/species/AMRO",
            "image_url":"https://static.avicommons.org/AMRO-abc-320.jpg",
            "creator":"A. Birder","license":"CC BY-SA 4.0",
            "license_url":"https://creativecommons.org/licenses/by-sa/4.0/"}]}"#;
        let attribution = Attribution::decode(json.as_bytes()).expect("attribution");
        assert_eq!(attribution.photos[0].creator, "A. Birder");
        assert_eq!(attribution.photos[0].license, "CC BY-SA 4.0");
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
