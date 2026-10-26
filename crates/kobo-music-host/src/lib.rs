//! Host-side score preparation for Music Stand: the shelf manifest codec and
//! page fitting. The CLI walks inputs and rasterizes PDFs; this crate keeps
//! the on-device format honest and testable on any platform.

use kobo_image::Picture;
use kobo_json::{ObjectBuilder, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MANIFEST: &str = "manifest.v1";
pub const MAX_MANIFEST: usize = 256 * 1024;
pub const MAX_SCORES: usize = 64;
pub const MAX_PAGES_PER_SCORE: usize = 400;
const FORMAT: &str = "musicstand-shelf";
/// Prepared pages are wider than the panel so zoom and half-page crops keep
/// real detail instead of scaling blur.
const PAGE_SCALE_NUMERATOR: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Panel {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Score {
    pub id: String,
    pub title: String,
    pub digest: String,
    pub pages: u32,
    pub width: u32,
    pub height: u32,
    pub added: u64,
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

impl Manifest {
    pub fn encode(&self) -> Vec<u8> {
        let scores: Vec<Value> = self
            .scores
            .iter()
            .map(|score| {
                ObjectBuilder::new()
                    .set("id", score.id.clone())
                    .set("title", score.title.clone())
                    .set("digest", score.digest.clone())
                    .set("pages", score.pages.to_string())
                    .set("width", score.width.to_string())
                    .set("height", score.height.to_string())
                    .set("added", score.added.to_string())
                    .build()
            })
            .collect();
        let failures: Vec<Value> = self
            .failures
            .iter()
            .map(|failure| {
                ObjectBuilder::new()
                    .set("input", failure.input.clone())
                    .set("reason", failure.reason.clone())
                    .build()
            })
            .collect();
        let root = ObjectBuilder::new()
            .set("format", FORMAT)
            .set("version", "1")
            .set("scores", scores)
            .set("failures", failures)
            .build();
        root.to_json().into_bytes()
    }

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
            let object = entry;
            let text = |key: &str| -> Result<String, String> {
                object
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
                digest: text("digest")?,
                pages: u32::try_from(number("pages")?)
                    .map_err(|_| "a shelf score has too many pages".to_owned())?,
                width: u32::try_from(number("width")?)
                    .map_err(|_| "a shelf score page is too wide".to_owned())?,
                height: u32::try_from(number("height")?)
                    .map_err(|_| "a shelf score page is too tall".to_owned())?,
                added: number("added")?,
            });
        }
        let mut failures = Vec::new();
        if let Some(entries) = root.get("failures").and_then(Value::as_array) {
            for entry in entries {
                let object = entry;
                let text = |key: &str| -> Result<String, String> {
                    object
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

/// Content digest for change detection, matching the Frame shelf's scheme.
pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// One score offered by the CLI after input walking and rasterization.
pub struct IncomingScore {
    pub title: String,
    pub digest: String,
    pub added: u64,
    pub pages: Vec<Picture>,
}

pub struct PreparedPage {
    pub score_id: String,
    pub index: u32,
    pub png: Vec<u8>,
}

pub struct Push {
    pub manifest: Manifest,
    pub pages: Vec<PreparedPage>,
    pub removed: Vec<Score>,
}

impl Push {
    pub fn page_name(score_id: &str, index: u32) -> String {
        format!("{score_id}-{index:03}.png")
    }
}

/// Fit a raw page for the shelf: grayscale, between one and two panel widths
/// wide so full-page view is crisp and zoom keeps detail, height capped.
pub fn fit_page(page: &Picture, panel: Panel) -> Result<Picture, String> {
    if panel.width == 0 || panel.height == 0 {
        return Err("Music Stand received unsupported panel dimensions".to_owned());
    }
    let grey = page.clone().into_grey_picture();
    let target = panel.width.saturating_mul(PAGE_SCALE_NUMERATOR);
    let fitted = if grey.width() > target {
        grey.fit(target, u32::MAX)
            .map_err(|error| format!("could not scale a score page: {error}"))?
    } else if grey.width() < panel.width {
        grey.fit_enlarging(panel.width, u32::MAX)
            .map_err(|error| format!("could not scale a score page: {error}"))?
    } else {
        grey
    };
    let pixels = u64::from(fitted.width()) * u64::from(fitted.height());
    if pixels > kobo_image::MAX_PIXELS {
        // Very tall pages: shrink until the picture budget holds the whole
        // page. Never crop; a cropped staff loses music.
        let scale = (kobo_image::MAX_PIXELS as f64 / pixels as f64).sqrt();
        let width = (u64::from(fitted.width()) as f64 * scale) as u32;
        return fitted
            .fit(width.max(1), u32::MAX)
            .map_err(|error| format!("could not fit a tall score page: {error}"));
    }
    Ok(fitted)
}

/// Merge incoming scores into the existing shelf: unchanged digests are kept,
/// new scores get fresh page files, and failures ride the manifest so the app
/// can report them honestly.
pub fn plan(
    existing: &Manifest,
    incoming: Vec<IncomingScore>,
    mut failures: Vec<ImportFailure>,
) -> Result<Push, String> {
    let old_by_digest: BTreeMap<&str, &Score> = existing
        .scores
        .iter()
        .map(|score| (score.digest.as_str(), score))
        .collect();
    let mut used_ids: BTreeSet<String> = existing
        .scores
        .iter()
        .map(|score| score.id.clone())
        .collect();
    let mut scores = Vec::new();
    let mut pages = Vec::new();
    for mut offer in incoming {
        if offer.pages.is_empty() {
            failures.push(ImportFailure {
                input: offer.title,
                reason: "no readable pages".to_owned(),
            });
            continue;
        }
        if offer.pages.len() > MAX_PAGES_PER_SCORE {
            failures.push(ImportFailure {
                input: offer.title,
                reason: format!("more than {MAX_PAGES_PER_SCORE} pages"),
            });
            continue;
        }
        if let Some(old) = old_by_digest.get(offer.digest.as_str()) {
            scores.push((*old).clone());
            continue;
        }
        let first = offer.pages.first().expect("non-empty");
        let width = first.width();
        let height = first.height();
        let page_count = u32::try_from(offer.pages.len()).unwrap_or(u32::MAX);
        let base = format!("score-{}", &offer.digest[..16.min(offer.digest.len())]);
        let mut id = base.clone();
        let mut suffix = 2_u32;
        while !used_ids.insert(id.clone()) {
            id = format!("{base}-{suffix}");
            suffix = suffix.saturating_add(1);
        }
        for (index, page) in offer.pages.drain(..).enumerate() {
            let index = u32::try_from(index).map_err(|_| "too many pages".to_owned())?;
            let grey = page.into_grey_picture();
            let png = kobo_image::encode_png_grey(grey.width(), grey.height(), grey.grey())
                .map_err(|error| format!("could not encode a score page: {error}"))?;
            pages.push(PreparedPage {
                score_id: id.clone(),
                index,
                png,
            });
        }
        scores.push(Score {
            id,
            title: offer.title,
            digest: offer.digest,
            pages: page_count,
            width,
            height,
            added: offer.added,
        });
        if scores.len() > MAX_SCORES {
            return Err(format!("Music Stand holds at most {MAX_SCORES} scores"));
        }
    }
    let wanted: BTreeSet<&str> = scores.iter().map(|score| score.id.as_str()).collect();
    let removed: Vec<Score> = existing
        .scores
        .iter()
        .filter(|score| !wanted.contains(score.id.as_str()))
        .cloned()
        .collect();
    Ok(Push {
        manifest: Manifest { scores, failures },
        pages,
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        Panel {
            width: 1072,
            height: 1448,
        }
    }

    fn grey_page(width: u32, height: u32, shade: u8) -> Picture {
        Picture::from_grey(width, height, vec![shade; (width * height) as usize]).expect("page")
    }

    #[test]
    fn manifest_round_trips_scores_and_failures() {
        let manifest = Manifest {
            scores: vec![Score {
                id: "score-abc".to_owned(),
                title: "Cello Suite No. 1".to_owned(),
                digest: "abc".to_owned(),
                pages: 3,
                width: 2144,
                height: 3032,
                added: 42,
            }],
            failures: vec![ImportFailure {
                input: "bad.pdf".to_owned(),
                reason: "no readable pages".to_owned(),
            }],
        };
        assert_eq!(Manifest::decode(&manifest.encode()), Ok(manifest));
    }

    #[test]
    fn manifest_rejects_wrong_format_and_oversize() {
        assert!(Manifest::decode(b"{}").is_err());
        assert!(Manifest::decode(&vec![b' '; MAX_MANIFEST + 1]).is_err());
    }

    #[test]
    fn fit_page_scales_wide_pages_down_and_narrow_pages_up() {
        let wide = fit_page(&grey_page(4096, 1500, 200), panel()).expect("fit");
        assert_eq!(wide.width(), 2144);
        let narrow = fit_page(&grey_page(600, 800, 200), panel()).expect("fit");
        assert_eq!(narrow.width(), 1072);
        let inside = fit_page(&grey_page(1500, 2000, 200), panel()).expect("fit");
        assert_eq!(inside.width(), 1500);
    }

    #[test]
    fn plan_keeps_unchanged_scores_and_removes_absent() {
        let existing = Manifest {
            scores: vec![
                Score {
                    id: "score-old".to_owned(),
                    title: "Kept".to_owned(),
                    digest: "same-digest-padded".to_owned(),
                    pages: 2,
                    width: 2144,
                    height: 3000,
                    added: 1,
                },
                Score {
                    id: "score-gone".to_owned(),
                    title: "Gone".to_owned(),
                    digest: "other-digest-pad".to_owned(),
                    pages: 1,
                    width: 2144,
                    height: 3000,
                    added: 1,
                },
            ],
            failures: Vec::new(),
        };
        let incoming = vec![
            IncomingScore {
                title: "Kept".to_owned(),
                digest: "same-digest-padded".to_owned(),
                added: 2,
                pages: vec![grey_page(2144, 2800, 1)],
            },
            IncomingScore {
                title: "New".to_owned(),
                digest: "brand-new-digest0".to_owned(),
                added: 3,
                pages: vec![grey_page(2144, 2800, 2), grey_page(2144, 2800, 3)],
            },
        ];
        let push = plan(&existing, incoming, Vec::new()).expect("plan");
        assert_eq!(push.manifest.scores.len(), 2);
        assert_eq!(push.manifest.scores[0].id, "score-old");
        assert_eq!(push.manifest.scores[1].id, "score-brand-new-digest");
        assert_eq!(push.pages.len(), 2);
        assert_eq!(Push::page_name("score-x", 7), "score-x-007.png");
        assert_eq!(push.removed.len(), 1);
        assert_eq!(push.removed[0].id, "score-gone");
    }

    #[test]
    fn plan_records_empty_scores_as_failures() {
        let push = plan(
            &Manifest::default(),
            vec![IncomingScore {
                title: "empty.pdf".to_owned(),
                digest: "d".to_owned(),
                added: 0,
                pages: Vec::new(),
            }],
            Vec::new(),
        )
        .expect("plan");
        assert!(push.manifest.scores.is_empty());
        assert_eq!(push.manifest.failures[0].input, "empty.pdf");
    }
}
