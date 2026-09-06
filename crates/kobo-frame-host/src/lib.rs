//! Bounded host-side preparation for the Frame application's private shelf.

use image::imageops::{overlay, resize, FilterType};
use image::{DynamicImage, GrayImage, ImageDecoder, ImageEncoder, ImageReader};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const DEFAULT_PANEL: Panel = Panel {
    width: 1072,
    height: 1448,
};
pub const MAX_PHOTOS: usize = 500;
pub const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_SOURCE_PIXELS: u64 = 50_000_000;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_CAPACITY: usize = 150 * 1024 * 1024;
pub const MANIFEST: &str = "manifest.v1";
pub const MANIFEST_HEADER: &str = "cobalt-frame-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Panel {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fit {
    #[default]
    Crop,
    Pad,
}

impl Fit {
    /// # Errors
    ///
    /// Returns an error unless `value` is `crop` or `pad`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "crop" => Ok(Self::Crop),
            "pad" => Ok(Self::Pad),
            _ => Err("--fit must be crop or pad".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Photo {
    pub id: String,
    pub digest: String,
    pub taken: u64,
    pub album: String,
    pub name: String,
}

impl Photo {
    #[must_use]
    pub fn shelf_name(&self) -> String {
        format!("{}.png", self.id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Manifest {
    pub photos: Vec<Photo>,
}

impl Manifest {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut output = format!("{MANIFEST_HEADER}\n");
        for photo in &self.photos {
            let _ = writeln!(
                output,
                "{}\t{}\t{}\t{}\t{}",
                photo.id,
                photo.digest,
                photo.taken,
                field(&photo.album),
                field(&photo.name)
            );
        }
        output.into_bytes()
    }

    /// # Errors
    ///
    /// Returns an error when the manifest is not the bounded Frame v1 format.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "Frame manifest is not UTF-8")?;
        let mut lines = text.lines();
        if lines.next() != Some(MANIFEST_HEADER) {
            return Err("Frame manifest has an unknown version".to_owned());
        }
        let mut photos = Vec::new();
        let mut ids = BTreeSet::new();
        for line in lines {
            let fields = line.split('\t').collect::<Vec<_>>();
            let [id, digest, taken, album, name] = fields.as_slice() else {
                return Err("Frame manifest has a malformed photo entry".to_owned());
            };
            if !valid_id(id)
                || digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !ids.insert((*id).to_owned())
            {
                return Err("Frame manifest has an invalid photo identity".to_owned());
            }
            let taken = taken
                .parse()
                .map_err(|_| "Frame manifest has an invalid date")?;
            photos.push(Photo {
                id: (*id).to_owned(),
                digest: (*digest).to_owned(),
                taken,
                album: (*album).to_owned(),
                name: (*name).to_owned(),
            });
        }
        if photos.len() > MAX_PHOTOS {
            return Err(format!(
                "Frame manifest exceeds the {MAX_PHOTOS}-photo limit"
            ));
        }
        Ok(Self { photos })
    }
}

#[derive(Clone, Debug)]
pub struct PreparedPhoto {
    pub photo: Photo,
    pub png: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct Push {
    pub manifest: Manifest,
    pub photos: Vec<PreparedPhoto>,
    pub removed: Vec<Photo>,
}

/// # Errors
///
/// Returns an error when the input, decoded image, prepared frame, or proposed
/// shelf exceeds one of Frame's explicit safety limits.
#[allow(clippy::too_many_lines)]
pub fn prepare(
    input: &Path,
    fit: Fit,
    existing: &Manifest,
    delete_missing: bool,
) -> Result<Push, String> {
    prepare_for_panel(input, fit, existing, delete_missing, DEFAULT_PANEL)
}

/// # Errors
///
/// Returns an error when the panel dimensions or prepared shelf are invalid.
#[allow(clippy::too_many_lines)]
pub fn prepare_for_panel(
    input: &Path,
    fit: Fit,
    existing: &Manifest,
    delete_missing: bool,
    panel: Panel,
) -> Result<Push, String> {
    if panel.width == 0
        || panel.height == 0
        || u64::from(panel.width) * u64::from(panel.height) > 8_000_000
    {
        return Err("Frame received unsupported panel dimensions".to_owned());
    }
    let paths = input_paths(input)?;
    if paths.len() > MAX_PHOTOS {
        return Err(format!(
            "{} has {} supported images; Frame accepts at most {MAX_PHOTOS}",
            input.display(),
            paths.len()
        ));
    }
    let old_by_digest = existing
        .photos
        .iter()
        .map(|photo| (photo.digest.as_str(), photo))
        .collect::<BTreeMap<_, _>>();
    let mut used_ids = existing
        .photos
        .iter()
        .map(|photo| photo.id.clone())
        .collect::<BTreeSet<_>>();
    let album = album_name(input);
    let mut seen_digests = BTreeSet::new();
    let mut prepared = Vec::new();
    for path in paths {
        let bytes = bounded_file(&path)?;
        let digest = blake3::hash(&bytes).to_hex().to_string();
        if !seen_digests.insert(digest.clone()) {
            continue;
        }
        if let Some(old) = old_by_digest.get(digest.as_str()) {
            prepared.push(PreparedPhoto {
                photo: (*old).clone(),
                png: None,
            });
            continue;
        }
        let image = decode(&path, &bytes)?;
        let png = encode(&fit_image(&image, fit, panel), panel)?;
        let base = format!("photo-{}", &digest[..16]);
        let mut id = base.clone();
        let mut suffix = 2_u32;
        while !used_ids.insert(id.clone()) {
            id = format!("{base}-{suffix}");
            suffix = suffix.saturating_add(1);
        }
        let taken = fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{} has no UTF-8 file name", path.display()))?
            .to_owned();
        prepared.push(PreparedPhoto {
            photo: Photo {
                id,
                digest,
                taken,
                album: album.clone(),
                name,
            },
            png: Some(png),
        });
    }
    let wanted = prepared
        .iter()
        .map(|prepared| prepared.photo.id.as_str())
        .collect::<BTreeSet<_>>();
    let removed = if delete_missing {
        existing
            .photos
            .iter()
            .filter(|photo| !wanted.contains(photo.id.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let mut final_photos = if delete_missing {
        prepared
            .iter()
            .map(|prepared| prepared.photo.clone())
            .collect()
    } else {
        let mut photos = existing.photos.clone();
        for incoming in &prepared {
            if !photos.iter().any(|photo| photo.id == incoming.photo.id) {
                photos.push(incoming.photo.clone());
            }
        }
        photos
    };
    final_photos.sort_by(|left, right| {
        left.taken
            .cmp(&right.taken)
            .then_with(|| left.id.cmp(&right.id))
    });
    if final_photos.len() > MAX_PHOTOS {
        return Err(format!(
            "this would keep {} photos; Frame's capacity is {MAX_PHOTOS}",
            final_photos.len()
        ));
    }
    let new_bytes = prepared
        .iter()
        .filter_map(|prepared| prepared.png.as_ref())
        .map(Vec::len)
        .sum::<usize>();
    if new_bytes > MAX_FRAME_CAPACITY {
        return Err("prepared photos exceed Frame's 150 MB capacity".to_owned());
    }
    Ok(Push {
        manifest: Manifest {
            photos: final_photos,
        },
        photos: prepared,
        removed,
    })
}

fn input_paths(input: &Path) -> Result<Vec<PathBuf>, String> {
    let metadata =
        fs::metadata(input).map_err(|error| format!("read {}: {error}", input.display()))?;
    let mut paths = Vec::new();
    if metadata.is_file() {
        supported(input)?;
        paths.push(input.to_path_buf());
    } else if metadata.is_dir() {
        collect(input, &mut paths)?;
    } else {
        return Err(format!(
            "{} is not a regular file or directory",
            input.display()
        ));
    }
    paths.sort();
    if paths.is_empty() {
        return Err(format!("{} has no supported images", input.display()));
    }
    Ok(paths)
}

fn collect(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", path.display()))?;
        if kind.is_dir() {
            collect(&path, paths)?;
        } else if kind.is_file() {
            match supported(&path) {
                Ok(()) => paths.push(path),
                Err(error) if is_heic(&path) => return Err(error),
                Err(_) => {}
            }
        }
    }
    Ok(())
}

fn supported(path: &Path) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if is_heic(path) {
        return Err(format!(
            "{} is HEIC/HEIF; convert it to JPEG or PNG first (Frame v1 deliberately does not add LGPL libheif)",
            path.display()
        ));
    }

    if matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp") {
        Ok(())
    } else {
        Err(format!(
            "{} is not a supported JPEG, PNG, GIF, or WebP image",
            path.display()
        ))
    }
}

fn is_heic(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "heic" | "heif"))
}

fn bounded_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "{} is {} MB; Frame reads at most {} MB per source",
            path.display(),
            metadata.len() / (1024 * 1024),
            MAX_SOURCE_BYTES / (1024 * 1024)
        ));
    }
    let file = fs::File::open(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!(
            "{} grew past Frame's {} MB source limit while it was read",
            path.display(),
            MAX_SOURCE_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

fn decode(path: &Path, bytes: &[u8]) -> Result<DynamicImage, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("decode {}: {error}", path.display()))?;
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_SOURCE_PIXELS {
        return Err(format!(
            "{} declares {pixels} pixels; Frame decodes at most {MAX_SOURCE_PIXELS}",
            path.display()
        ));
    }
    let orientation = decoder
        .orientation()
        .map_err(|error| format!("read orientation from {}: {error}", path.display()))?;
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("decode {}: {error}", path.display()))?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn fit_image(image: &DynamicImage, fit: Fit, panel: Panel) -> GrayImage {
    let source = image.to_luma8();
    let (width, height) = source.dimensions();
    match fit {
        Fit::Crop => {
            let width_limited = u64::from(panel.width) * u64::from(height)
                >= u64::from(panel.height) * u64::from(width);
            let (target_width, target_height) = if width_limited {
                (
                    panel.width,
                    u32::try_from(
                        (u64::from(panel.width) * u64::from(height)).div_ceil(u64::from(width)),
                    )
                    .unwrap_or(panel.height),
                )
            } else {
                (
                    u32::try_from(
                        (u64::from(panel.height) * u64::from(width)).div_ceil(u64::from(height)),
                    )
                    .unwrap_or(panel.width),
                    panel.height,
                )
            };
            let scaled = resize(&source, target_width, target_height, FilterType::Lanczos3);
            image::imageops::crop_imm(
                &scaled,
                (target_width - panel.width) / 2,
                (target_height - panel.height) / 2,
                panel.width,
                panel.height,
            )
            .to_image()
        }
        Fit::Pad => {
            let scale_width = u64::from(panel.width) * u64::from(height);
            let scale_height = u64::from(panel.height) * u64::from(width);
            let (target_width, target_height) = if scale_width <= scale_height {
                (
                    panel.width,
                    u32::try_from(scale_width / u64::from(width))
                        .unwrap_or(panel.height)
                        .max(1),
                )
            } else {
                (
                    u32::try_from(scale_height / u64::from(height))
                        .unwrap_or(panel.width)
                        .max(1),
                    panel.height,
                )
            };
            let scaled = resize(&source, target_width, target_height, FilterType::Lanczos3);
            let mut canvas = GrayImage::from_pixel(panel.width, panel.height, image::Luma([255]));
            overlay(
                &mut canvas,
                &scaled,
                i64::from((panel.width - target_width) / 2),
                i64::from((panel.height - target_height) / 2),
            );
            canvas
        }
    }
}

fn encode(image: &GrayImage, panel: Panel) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            image.as_raw(),
            panel.width,
            panel.height,
            image::ExtendedColorType::L8,
        )
        .map_err(|error| format!("encode Frame image: {error}"))?;
    if png.len() > MAX_FRAME_BYTES {
        return Err(format!(
            "prepared Frame image is {} MB; the device accepts at most {} MB",
            png.len() / (1024 * 1024),
            MAX_FRAME_BYTES / (1024 * 1024)
        ));
    }
    Ok(png)
}

fn valid_id(id: &str) -> bool {
    id.starts_with("photo-")
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn field(value: &str) -> String {
    value
        .replace(['\t', '\n', '\r'], " ")
        .chars()
        .take(160)
        .collect()
}

fn album_name(input: &Path) -> String {
    input
        .file_name()
        .or_else(|| input.parent().and_then(Path::file_name))
        .and_then(|name| name.to_str())
        .map_or_else(|| "Photos".to_owned(), field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgb};

    fn fixture(path: &Path, width: u32, height: u32) {
        ImageBuffer::<Rgb<u8>, _>::from_fn(width, height, |x, y| {
            Rgb([
                u8::try_from(x).unwrap_or(0),
                u8::try_from(y).unwrap_or(0),
                80,
            ])
        })
        .save(path)
        .expect("fixture");
    }

    #[test]
    fn manifest_round_trips_and_refuses_bad_ids() {
        let manifest = Manifest {
            photos: vec![Photo {
                id: "photo-0123456789abcdef".into(),
                digest: "a".repeat(64),
                taken: 42,
                album: "Family".into(),
                name: "one.png".into(),
            }],
        };
        assert_eq!(
            Manifest::decode(&manifest.encode()).expect("decode"),
            manifest
        );
        assert!(Manifest::decode(b"cobalt-frame-v1\n../bad\taaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\t0\ta\tb\n").is_err());
    }

    #[test]
    fn crop_and_pad_make_panel_sized_pngs() {
        let root = std::env::temp_dir().join(format!("frame-host-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("dir");
        let input = root.join("landscape.png");
        fixture(&input, 400, 200);
        for fit in [Fit::Crop, Fit::Pad] {
            let push = prepare(&input, fit, &Manifest::default(), false).expect("prepare");
            let png = push.photos[0].png.as_ref().expect("new image");
            let decoded = image::load_from_memory(png).expect("output");
            assert_eq!(
                decoded.dimensions(),
                (DEFAULT_PANEL.width, DEFAULT_PANEL.height)
            );
            assert!(png.len() <= MAX_FRAME_BYTES);
        }

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn preparation_uses_the_selected_reader_panel() {
        let root =
            std::env::temp_dir().join(format!("frame-host-other-panel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("dir");
        let input = root.join("portrait.png");
        fixture(&input, 200, 400);
        let panel = Panel {
            width: 1264,
            height: 1680,
        };
        let push =
            prepare_for_panel(&input, Fit::Crop, &Manifest::default(), false, panel).expect("push");
        let decoded =
            image::load_from_memory(push.photos[0].png.as_ref().expect("photo")).expect("decode");
        assert_eq!(decoded.dimensions(), (panel.width, panel.height));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unchanged_files_keep_their_identity_and_delete_is_explicit() {
        let root = std::env::temp_dir().join(format!("frame-host-identity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("dir");
        let input = root.join("one.png");
        fixture(&input, 10, 10);
        let first = prepare(&input, Fit::Crop, &Manifest::default(), false).expect("first");
        let second = prepare(&input, Fit::Crop, &first.manifest, false).expect("second");
        assert_eq!(second.photos[0].photo.id, first.photos[0].photo.id);
        assert!(second.photos[0].png.is_none());
        let extra = Photo {
            id: "photo-ffffffffffffffff".into(),
            digest: "f".repeat(64),
            taken: 0,
            album: "Old".into(),
            name: "old.png".into(),
        };
        let existing = Manifest {
            photos: [first.manifest.photos[0].clone(), extra.clone()].into(),
        };
        assert_eq!(
            prepare(&input, Fit::Crop, &existing, false)
                .expect("merge")
                .manifest
                .photos
                .len(),
            2
        );
        assert_eq!(
            prepare(&input, Fit::Crop, &existing, true)
                .expect("replace")
                .removed,
            vec![extra]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn heic_is_explained_before_it_is_decoded() {
        assert!(supported(Path::new("portrait.heic"))
            .expect_err("refuse")
            .contains("convert it to JPEG or PNG"));
    }
}
