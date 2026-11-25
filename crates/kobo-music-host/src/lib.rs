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

/// The most pixels a page may decode to on the computer.
///
/// The reader's own ceiling, [`kobo_image::MAX_PIXELS`], is sized for a
/// single core and half a gigabyte, and a letter page scanned at 300 dpi is
/// already a third over it. The computer has the memory, so it reads the page
/// whole and sends the reader something that fits. Fifty million is an A3
/// page at 400 dpi, with room to spare, and still refuses a header claiming
/// a picture no score is.
pub const MAX_SOURCE_PIXELS: u64 = 50_000_000;

/// Reads one page image on the computer and fits it for the shelf.
///
/// Pages the reader could decode itself take the reader's own path, so what
/// it receives for them is what it always did. A page past the reader's limit,
/// a 300 dpi scan or a large PDF page, is read here at full size and scaled
/// in one step to the width [`fit_page`] would give it, and inside the
/// reader's pixel budget, then handed to [`fit_page`] like any other.
///
/// # Errors
///
/// The reason the page could not be used, in words for the person pushing.
pub fn decode_page(bytes: &[u8], panel: Panel) -> Result<Picture, String> {
    match kobo_image::decode(bytes) {
        Ok(picture) => return fit_page(&picture, panel),
        Err(
            kobo_image::ImageError::TooManyPixels { .. }
            | kobo_image::ImageError::TooManyBytes { .. },
        ) => {}
        Err(error) => return Err(format!("could not be decoded: {error}")),
    }
    if panel.width == 0 || panel.height == 0 {
        return Err("Music Stand received unsupported panel dimensions".to_owned());
    }
    let undecodable = |error: image::ImageError| format!("could not be decoded: {error}");
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("could not be decoded: {error}"))?
        .into_decoder()
        .map_err(undecodable)?;
    let (width, height) = image::ImageDecoder::dimensions(&decoder);
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_SOURCE_PIXELS {
        return Err(format!(
            "could not be decoded: the page claims {pixels} pixels, and {MAX_SOURCE_PIXELS} is the most read"
        ));
    }
    let orientation = image::ImageDecoder::orientation(&mut decoder).map_err(undecodable)?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(undecodable)?;
    image.apply_orientation(orientation);
    // Transparency onto white paper and colour to luminance, the way the
    // reader's own decode does it, so a big page and a small one from the
    // same scanner come out the same grey.
    let luma = image.to_luma_alpha8();
    drop(image);
    let (width, height) = luma.dimensions();
    let paper = image::GrayImage::from_fn(width, height, |x, y| {
        let pixel = luma.get_pixel(x, y);
        let alpha = u32::from(pixel[1]);
        let on_paper = (u32::from(pixel[0]) * alpha + 255 * (255 - alpha) + 127) / 255;
        image::Luma([u8::try_from(on_paper).unwrap_or(255)])
    });
    drop(luma);
    let (target_width, target_height) = within_budget(width, height, panel);
    let scaled = image::imageops::resize(
        &paper,
        target_width,
        target_height,
        image::imageops::FilterType::Lanczos3,
    );
    let picture = Picture::from_grey(target_width, target_height, scaled.into_raw())
        .map_err(|error| format!("could not scale a score page: {error}"))?;
    fit_page(&picture, panel)
}

/// The size a page too big for the reader is scaled to: no wider than
/// [`fit_page`] makes a page, and no more pixels than the reader decodes.
fn within_budget(width: u32, height: u32, panel: Panel) -> (u32, u32) {
    let widest = panel.width.saturating_mul(PAGE_SCALE_NUMERATOR);
    let mut target = width.min(widest).max(1);
    let tall = |across: u32| {
        u32::try_from(u64::from(height) * u64::from(across) / u64::from(width))
            .unwrap_or(u32::MAX)
            .max(1)
    };
    let pixels = u64::from(target) * u64::from(tall(target));
    if pixels > kobo_image::MAX_PIXELS {
        // Never crop; a cropped staff loses music. Rounded down, so the
        // page lands inside the budget rather than a row over it.
        let scale = (kobo_image::MAX_PIXELS as f64 / pixels as f64).sqrt();
        target = ((f64::from(target) * scale) as u32).max(1);
        while u64::from(target) * u64::from(tall(target)) > kobo_image::MAX_PIXELS && target > 1 {
            target -= 1;
        }
    }
    (target, tall(target))
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
///
/// A push adds to the shelf unless `replace` asks for the shelf to become
/// exactly what was sent. Adding is the default because sending one more
/// score is what a push nearly always means, and a shelf that silently lost
/// every other score to a push of one was the opposite of what anybody
/// expected. A score sent again under the same title takes the place of the
/// one it updates rather than standing beside it.
pub fn plan(
    existing: &Manifest,
    incoming: Vec<IncomingScore>,
    mut failures: Vec<ImportFailure>,
    replace: bool,
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
    if !replace {
        let sent: BTreeSet<&str> = scores.iter().map(|score| score.title.as_str()).collect();
        let kept: Vec<Score> = existing
            .scores
            .iter()
            .filter(|old| {
                !sent.contains(old.title.as_str()) && !scores.iter().any(|score| score.id == old.id)
            })
            .cloned()
            .collect();
        scores.splice(0..0, kept);
        if scores.len() > MAX_SCORES {
            return Err(format!(
                "Music Stand holds at most {MAX_SCORES} scores. Remove some with kobo musicstand rm, or send this push with --replace."
            ));
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

    /// A page as a scanner writes it: grey PNG, a dark staff line across a
    /// white page, so scaling has something to keep.
    fn scanned_png(width: u32, height: u32) -> Vec<u8> {
        let page = image::GrayImage::from_fn(width, height, |_, y| {
            image::Luma([if y % 40 < 3 { 20 } else { 250 }])
        });
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(page)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("png");
        png
    }

    #[test]
    fn a_letter_page_scanned_at_300_dpi_is_scaled_to_fit_the_reader_rather_than_refused() {
        let png = scanned_png(2550, 3300);
        assert!(
            kobo_image::decode(&png).is_err(),
            "the reader's own limit refuses it"
        );
        let page = decode_page(&png, panel()).expect("the computer reads it");
        assert_eq!(page.width(), 2144, "as wide as any other prepared page");
        assert_eq!(page.height(), 2774);
        assert!(u64::from(page.width()) * u64::from(page.height()) <= kobo_image::MAX_PIXELS);
        assert!(kobo_image::decode(
            &kobo_image::encode_png_grey(page.width(), page.height(), page.grey()).expect("png")
        )
        .is_ok());
    }

    #[test]
    fn a_tall_large_page_lands_inside_the_readers_pixel_budget_without_cropping() {
        // An A3 page rendered at 200 dpi, then one far taller than it is wide.
        for (width, height) in [(2339, 3308), (2400, 9000)] {
            let page = decode_page(&scanned_png(width, height), panel()).expect("page");
            assert!(u64::from(page.width()) * u64::from(page.height()) <= kobo_image::MAX_PIXELS);
            let source = f64::from(height) / f64::from(width);
            let kept = f64::from(page.height()) / f64::from(page.width());
            assert!(
                (source - kept).abs() < 0.01,
                "{width}x{height} kept its shape"
            );
        }
    }

    #[test]
    fn a_page_the_reader_could_decode_is_prepared_exactly_as_before() {
        let png = scanned_png(1700, 2200);
        let before = fit_page(&kobo_image::decode(&png).expect("decode"), panel()).expect("fit");
        assert_eq!(decode_page(&png, panel()).expect("page"), before);
    }

    #[test]
    fn a_file_that_is_not_a_picture_says_so() {
        let reason = decode_page(b"not a page", panel()).unwrap_err();
        assert!(reason.starts_with("could not be decoded"), "{reason}");
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
        let push = plan(&existing, incoming, Vec::new(), true).expect("plan");
        assert_eq!(push.manifest.scores.len(), 2);
        assert_eq!(push.manifest.scores[0].id, "score-old");
        assert_eq!(push.manifest.scores[1].id, "score-brand-new-digest");
        assert_eq!(push.pages.len(), 2);
        assert_eq!(Push::page_name("score-x", 7), "score-x-007.png");
        assert_eq!(push.removed.len(), 1);
        assert_eq!(push.removed[0].id, "score-gone");
    }

    fn score(id: &str, title: &str, digest: &str) -> Score {
        Score {
            id: id.to_owned(),
            title: title.to_owned(),
            digest: digest.to_owned(),
            pages: 1,
            width: 2144,
            height: 3000,
            added: 1,
        }
    }

    #[test]
    fn a_push_adds_to_the_shelf_and_keeps_every_score_it_did_not_mention() {
        let existing = Manifest {
            scores: vec![score("score-a", "Prelude", "aaaa-digest-padded")],
            failures: Vec::new(),
        };
        let push = plan(
            &existing,
            vec![IncomingScore {
                title: "Sarabande".to_owned(),
                digest: "bbbb-digest-padded".to_owned(),
                added: 2,
                pages: vec![grey_page(2144, 2800, 1)],
            }],
            Vec::new(),
            false,
        )
        .expect("plan");
        let titles = push
            .manifest
            .scores
            .iter()
            .map(|score| score.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["Prelude", "Sarabande"]);
        assert!(push.removed.is_empty());
    }

    #[test]
    fn a_score_sent_again_under_its_title_replaces_the_old_copy_instead_of_doubling() {
        let existing = Manifest {
            scores: vec![
                score("score-a", "Prelude", "aaaa-digest-padded"),
                score("score-c", "Gigue", "cccc-digest-padded"),
            ],
            failures: Vec::new(),
        };
        let push = plan(
            &existing,
            vec![IncomingScore {
                title: "Prelude".to_owned(),
                digest: "dddd-digest-padded".to_owned(),
                added: 2,
                pages: vec![grey_page(2144, 2800, 1)],
            }],
            Vec::new(),
            false,
        )
        .expect("plan");
        assert_eq!(push.manifest.scores.len(), 2);
        assert_eq!(push.removed.len(), 1);
        assert_eq!(push.removed[0].id, "score-a");
        assert!(push.manifest.scores.iter().any(|s| s.title == "Gigue"));
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
            false,
        )
        .expect("plan");
        assert!(push.manifest.scores.is_empty());
        assert_eq!(push.manifest.failures[0].input, "empty.pdf");
    }
}
