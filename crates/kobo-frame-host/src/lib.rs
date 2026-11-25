//! Bounded host-side preparation for the Frame application's private shelf.

use image::imageops::{overlay, resize, FilterType};
use image::{DynamicImage, ImageBuffer, ImageDecoder, ImageEncoder, ImageReader};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const DEFAULT_PANEL: Panel = Panel {
    width: 1072,
    height: 1448,
    colour: false,
};
/// The JPEG quality a colour photograph is prepared at. High enough that the
/// compression is not what a Kaleido panel's own filter and sixteen levels
/// per channel will hide anyway, low enough to keep a full panel well under
/// the shelf ceiling.
pub const COLOUR_JPEG_QUALITY: u8 = 88;
pub const MAX_PHOTOS: usize = 500;
pub const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_SOURCE_PIXELS: u64 = 50_000_000;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_CAPACITY: usize = 150 * 1024 * 1024;
pub const MANIFEST: &str = "manifest.v1";
pub const MANIFEST_HEADER: &str = "cobalt-frame-v1";
/// Sidecar recording each photo's panel fit; older app builds ignore it.
pub const FIT_MANIFEST: &str = "fit.v1";
/// Sidecar recording the digest of each pushed photo's shelf bytes, so the
/// reader can verify a transfer; older app builds ignore it.
pub const DIGEST_MANIFEST: &str = "digests.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Panel {
    pub width: u32,
    pub height: u32,
    /// Whether the reader's panel shows colour, from its profile. A
    /// photograph for a colour reader keeps its colour; one for a greyscale
    /// reader is reduced to grey here, on the computer, as it always was.
    pub colour: bool,
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

/// Encode the per-photo fit sidecar as `id<TAB>crop|pad` lines, ordered by id.
#[must_use]
pub fn encode_fit_map(map: &BTreeMap<String, Fit>) -> Vec<u8> {
    let mut output = String::new();
    for (id, fit) in map {
        let value = match fit {
            Fit::Crop => "crop",
            Fit::Pad => "pad",
        };
        let _ = writeln!(output, "{id}\t{value}");
    }
    output.into_bytes()
}

/// Decode a fit sidecar, skipping lines that are not `id<TAB>crop|pad`.
#[must_use]
pub fn decode_fit_map(bytes: &[u8]) -> BTreeMap<String, Fit> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (id, fit) = line.split_once('\t')?;
            if !valid_id(id) {
                return None;
            }
            Fit::parse(fit).ok().map(|fit| (id.to_owned(), fit))
        })
        .collect()
}

/// Encode the transfer-digest sidecar as `id<TAB>hex` lines, ordered by id.
#[must_use]
pub fn encode_digest_map(map: &BTreeMap<String, String>) -> Vec<u8> {
    let mut output = String::new();
    for (id, digest) in map {
        let _ = writeln!(output, "{id}\t{digest}");
    }
    output.into_bytes()
}

/// Decode a transfer-digest sidecar, skipping lines that are not
/// `id<TAB>64 hex digits`.
#[must_use]
pub fn decode_digest_map(bytes: &[u8]) -> BTreeMap<String, String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (id, digest) = line.split_once('\t')?;
            (valid_id(id)
                && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| (id.to_owned(), digest.to_owned()))
        })
        .collect()
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
    /// The bytes of the shelf entry, when this push carries them: a PNG for a
    /// greyscale reader, a JPEG for a colour one, under the entry's `.png`
    /// name either way; see `encode`.
    pub png: Option<Vec<u8>>,
    /// Digest of the prepared shelf bytes; `None` when the photo was
    /// already on the shelf and its bytes were not re-prepared.
    pub shelf_digest: Option<String>,
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
    prepare_for_panel_excluding(
        input,
        fit,
        existing,
        delete_missing,
        panel,
        &BTreeSet::new(),
    )
}

/// The same preparation, skipping input files by relative name. A publisher
/// that keeps its shelf inside the source folder uses this to leave its own
/// manifest, sidecars and previously prepared photos out of the next walk.
///
/// # Errors
///
/// Returns an error when the panel dimensions or prepared shelf are invalid.
#[allow(clippy::too_many_lines)]
pub fn prepare_for_panel_excluding(
    input: &Path,
    fit: Fit,
    existing: &Manifest,
    delete_missing: bool,
    panel: Panel,
    exclude: &BTreeSet<String>,
) -> Result<Push, String> {
    if panel.width == 0
        || panel.height == 0
        || u64::from(panel.width) * u64::from(panel.height) > 8_000_000
    {
        return Err("Frame received unsupported panel dimensions".to_owned());
    }
    let paths = input_paths_excluding(input, exclude)?;
    if paths.is_empty() && !delete_missing {
        return Err(format!("{} has no supported images", input.display()));
    }
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
            // A photo already on the shelf may carry the date its file was
            // copied, from a push before the camera's own date was read, so
            // pushing it again puts the camera's date right.
            let mut photo = (*old).clone();
            if let Some(taken) = exif_taken(&bytes) {
                photo.taken = taken;
            }
            prepared.push(PreparedPhoto {
                photo,
                png: None,
                shelf_digest: None,
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
        // The reader shows this as the day the photo was taken and orders
        // the shelf by it, so the camera's date comes first. The file's
        // modified time is only a last resort: a folder copied off a phone
        // today has every file modified today.
        let taken = exif_taken(&bytes).unwrap_or_else(|| {
            fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs())
        });
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{} has no UTF-8 file name", path.display()))?
            .to_owned();
        let shelf_digest = blake3::hash(&png).to_hex().to_string();
        prepared.push(PreparedPhoto {
            photo: Photo {
                id,
                digest,
                taken,
                album: album.clone(),
                name,
            },
            png: Some(png),
            shelf_digest: Some(shelf_digest),
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
            if let Some(photo) = photos
                .iter_mut()
                .find(|photo| photo.id == incoming.photo.id)
            {
                photo.taken = incoming.photo.taken;
            } else {
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

fn input_paths_excluding(input: &Path, exclude: &BTreeSet<String>) -> Result<Vec<PathBuf>, String> {
    let metadata =
        fs::metadata(input).map_err(|error| format!("read {}: {error}", input.display()))?;
    let mut paths = Vec::new();
    if metadata.is_file() {
        supported(input)?;
        paths.push(input.to_path_buf());
    } else if metadata.is_dir() {
        collect(input, &mut paths)?;
        paths.retain(|path| {
            path.strip_prefix(input)
                .ok()
                .and_then(|relative| relative.to_str())
                .is_none_or(|relative| !exclude.contains(relative))
        });
    } else {
        return Err(format!(
            "{} is not a regular file or directory",
            input.display()
        ));
    }
    paths.sort();
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

/// When the camera says the photo was taken, as seconds since the epoch.
///
/// The image decoder already finds the Exif block in a JPEG's APP1 segment,
/// a PNG's `eXIf` chunk and a WebP's `EXIF` chunk, reading headers only; the
/// dates inside are read here by hand rather than with another crate. The
/// moment the shutter opened wins, then the moment the file was made, then
/// the last time software saved it. Exif writes the camera's wall clock with
/// no zone, and it is kept as that wall clock: the reader prints the date of
/// the stored instant without a zone, so it shows the day on the camera.
fn exif_taken(bytes: &[u8]) -> Option<u64> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let exif = decoder.exif_metadata().ok()??;
    // Some writers keep the APP1 identifier in a PNG's chunk as well.
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(&exif);
    let ifd = Tiff::new(tiff)?;
    let first = ifd.u32_at(4)?;
    let sub = ifd
        .entry(first, EXIF_IFD_POINTER)
        .and_then(|(kind, _, value)| (kind == 4 || kind == 13).then_some(value));
    [
        sub.and_then(|sub| ifd.date(sub, DATE_TIME_ORIGINAL)),
        sub.and_then(|sub| ifd.date(sub, DATE_TIME_DIGITIZED)),
        ifd.date(first, DATE_TIME),
    ]
    .into_iter()
    .flatten()
    .next()
}

const DATE_TIME: u16 = 0x0132;
const EXIF_IFD_POINTER: u16 = 0x8769;
const DATE_TIME_ORIGINAL: u16 = 0x9003;
const DATE_TIME_DIGITIZED: u16 = 0x9004;

/// A TIFF block's bytes and byte order, read with every offset checked, since
/// the block comes from whatever file was pushed.
struct Tiff<'a> {
    bytes: &'a [u8],
    little: bool,
}

impl<'a> Tiff<'a> {
    fn new(bytes: &'a [u8]) -> Option<Self> {
        let little = match bytes.get(..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Self { bytes, little })
    }

    fn u16_at(&self, offset: usize) -> Option<u16> {
        let raw: [u8; 2] = self
            .bytes
            .get(offset..offset.checked_add(2)?)?
            .try_into()
            .ok()?;
        Some(if self.little {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        })
    }

    fn u32_at(&self, offset: usize) -> Option<usize> {
        let raw: [u8; 4] = self
            .bytes
            .get(offset..offset.checked_add(4)?)?
            .try_into()
            .ok()?;
        let value = if self.little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        };
        usize::try_from(value).ok()
    }

    /// The type, count and value-or-offset of a tag in the directory at
    /// `ifd`.
    fn entry(&self, ifd: usize, tag: u16) -> Option<(u16, usize, usize)> {
        let count = self.u16_at(ifd)?;
        (0..usize::from(count)).find_map(|index| {
            let at = ifd.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
            (self.u16_at(at)? == tag).then_some(())?;
            Some((
                self.u16_at(at + 2)?,
                self.u32_at(at + 4)?,
                self.u32_at(at + 8)?,
            ))
        })
    }

    /// An Exif date tag, `YYYY:MM:DD HH:MM:SS`, as epoch seconds. A camera
    /// with no clock set writes zeros or blanks, which count as no date.
    fn date(&self, ifd: usize, tag: u16) -> Option<u64> {
        let (kind, count, offset) = self.entry(ifd, tag)?;
        if kind != 2 || count < 19 {
            return None;
        }
        let text = self.bytes.get(offset..offset.checked_add(19)?)?;
        let number = |range: std::ops::Range<usize>| -> Option<u64> {
            let digits = text.get(range)?;
            digits.iter().all(u8::is_ascii_digit).then_some(())?;
            std::str::from_utf8(digits).ok()?.parse().ok()
        };
        let year = number(0..4)?;
        let month = number(5..7)?;
        let day = number(8..10)?;
        let hour = number(11..13)?;
        let minute = number(14..16)?;
        let second = number(17..19)?;
        if !(1970..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return None;
        }
        Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
    }
}

/// Days from 1970-01-01 to a civil date in or after 1970, by Howard
/// Hinnant's algorithm, the inverse of the one the reader prints dates with.
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Fits the photograph to the panel, in colour for a panel that shows it and
/// in grey for one that does not.
///
/// The geometry is the same either way; only the pixel type differs, so the
/// work is done once, generically, and the caller gets whichever plane the
/// reader can use. A grey reader never pays for colour it cannot show, and a
/// colour reader is not handed a photograph with the colour already gone.
fn fit_image(image: &DynamicImage, fit: Fit, panel: Panel) -> DynamicImage {
    if panel.colour {
        fit_buffer(&image.to_rgb8(), fit, panel, image::Rgb([255, 255, 255])).into()
    } else {
        fit_buffer(&image.to_luma8(), fit, panel, image::Luma([255])).into()
    }
}

fn fit_buffer<P>(
    source: &ImageBuffer<P, Vec<u8>>,
    fit: Fit,
    panel: Panel,
    paper: P,
) -> ImageBuffer<P, Vec<u8>>
where
    P: image::Pixel<Subpixel = u8> + 'static,
{
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
            let scaled = resize(source, target_width, target_height, FilterType::Lanczos3);
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
            let scaled = resize(source, target_width, target_height, FilterType::Lanczos3);
            let mut canvas = ImageBuffer::from_pixel(panel.width, panel.height, paper);
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

/// Writes the fitted photograph as the shelf will hold it.
///
/// Grey goes out as a PNG, exactly as it always has. Colour goes out as a
/// JPEG at a quality a photograph does not show, because a colour PNG of a
/// full panel runs to several megabytes and would fail the ceiling below for
/// most photographs, and because the shelf is 150 MB for an album. The entry
/// keeps the `.png` name whichever it holds: the application asks for the
/// entry by that name, every build of it ever shipped reads the bytes rather
/// than the name, and a reader with an older Frame still shows the picture,
/// in grey.
fn encode(image: &DynamicImage, panel: Panel) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    match image {
        DynamicImage::ImageRgb8(colour) => {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, COLOUR_JPEG_QUALITY)
                .write_image(
                    colour.as_raw(),
                    panel.width,
                    panel.height,
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|error| format!("encode Frame image: {error}"))?;
        }
        other => {
            let grey = other.to_luma8();
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(
                    grey.as_raw(),
                    panel.width,
                    panel.height,
                    image::ExtendedColorType::L8,
                )
                .map_err(|error| format!("encode Frame image: {error}"))?;
        }
    }
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(format!(
            "prepared Frame image is {} MB; the device accepts at most {} MB",
            bytes.len() / (1024 * 1024),
            MAX_FRAME_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
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

/// The album a push files its photos under: the folder they came from.
///
/// A single photo belongs to the folder it was sent from too. Named after the
/// file itself, a one-photo push became an album called `IMG_2041.jpg`,
/// which reads as a file name rather than an album.
fn album_name(input: &Path) -> String {
    let resolved = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let folder = if resolved.is_file() {
        resolved.parent().map(Path::to_path_buf)
    } else {
        Some(resolved)
    };
    folder
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map_or_else(|| "Photos".to_owned(), field)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_single_photo_is_filed_under_the_folder_it_came_from() {
        let folder = std::env::temp_dir().join(format!("Summer holiday {}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        let photo = folder.join("IMG_2041.jpg");
        std::fs::write(&photo, b"x").expect("photo");
        let expected = folder
            .file_name()
            .and_then(|name| name.to_str())
            .expect("name")
            .to_owned();
        assert_eq!(super::album_name(&photo), expected);
        assert_eq!(super::album_name(&folder), expected);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn digest_map_round_trips() {
        let digest = "a".repeat(64);
        let map = BTreeMap::from([("photo-aaaa".to_owned(), digest.clone())]);
        let encoded = encode_digest_map(&map);
        assert_eq!(encoded, format!("photo-aaaa\t{digest}\n").into_bytes());
        assert_eq!(decode_digest_map(&encoded), map);
        assert!(decode_digest_map(b"photo-aaaa\tnothex\n").is_empty());
        assert!(decode_digest_map(b"../bad\tabcdef\n").is_empty());
    }

    #[test]
    fn fit_map_round_trips() {
        let mut map = std::collections::BTreeMap::new();
        map.insert("photo-bbbb".to_owned(), Fit::Pad);
        map.insert("photo-aaaa".to_owned(), Fit::Crop);
        let encoded = encode_fit_map(&map);
        assert_eq!(encoded, b"photo-aaaa\tcrop\nphoto-bbbb\tpad\n".to_vec());
        assert_eq!(decode_fit_map(&encoded), map);
    }

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
    fn a_colour_panel_keeps_the_photographs_colour_and_a_grey_one_does_not() {
        let root = std::env::temp_dir().join(format!("frame-host-colour-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("dir");
        let input = root.join("landscape.png");
        fixture(&input, 400, 200);
        let colour = Panel {
            colour: true,
            ..DEFAULT_PANEL
        };
        for fit in [Fit::Crop, Fit::Pad] {
            let push = prepare_for_panel(&input, fit, &Manifest::default(), false, colour)
                .expect("prepare");
            let bytes = push.photos[0].png.as_ref().expect("new image");
            let decoded = image::load_from_memory(bytes).expect("output");
            assert_eq!(decoded.dimensions(), (colour.width, colour.height));
            assert!(bytes.len() <= MAX_FRAME_BYTES);
            // The gradient fixture is coloured, so a prepared photograph that
            // kept its colour has pixels whose channels differ.
            let rgb = decoded.to_rgb8();
            assert!(
                rgb.pixels()
                    .any(|pixel| pixel[0] != pixel[1] || pixel[1] != pixel[2]),
                "the colour was lost on the way to a colour panel"
            );
            // And it is a JPEG, not a PNG, which is what keeps it under the
            // ceiling: the entry's name says png and the bytes say otherwise,
            // deliberately; see `encode`.
            assert_eq!(&bytes[..2], &[0xFF, 0xD8], "not a JPEG");
        }
        let grey = prepare_for_panel(
            &input,
            Fit::Crop,
            &Manifest::default(),
            false,
            DEFAULT_PANEL,
        )
        .expect("prepare");
        let bytes = grey.photos[0].png.as_ref().expect("new image");
        assert_eq!(
            &bytes[..4],
            b"\x89PNG",
            "a grey reader's photograph is still a PNG"
        );
        assert_eq!(
            image::load_from_memory(bytes).expect("output").color(),
            image::ColorType::L8
        );
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
            colour: false,
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

    /// A TIFF block in either byte order: `DateTime` in the first directory
    /// and, when given, `DateTimeOriginal` in the Exif directory it points to.
    fn exif_block(little: bool, modified: &str, original: Option<&str>) -> Vec<u8> {
        let u16b = |value: u16| {
            if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let u32b = |value: u32| {
            if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let entry = |out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
            out.extend(u16b(tag));
            out.extend(u16b(kind));
            out.extend(u32b(count));
            out.extend(u32b(value));
        };
        // Header 8, first directory 2 + 2 entries + next 4 = 30, sub
        // directory 2 + 1 entry + next 4 = 18, then the two strings.
        let (first, sub, strings) = (8_u32, 38_u32, 56_u32);
        let mut out = if little {
            b"II*\0".to_vec()
        } else {
            b"MM\0*".to_vec()
        };
        out.extend(u32b(first));
        out.extend(u16b(2));
        entry(&mut out, 0x0132, 2, 20, strings);
        entry(&mut out, 0x8769, 4, 1, sub);
        out.extend(u32b(0));
        out.extend(u16b(u16::from(original.is_some())));
        if original.is_some() {
            entry(&mut out, 0x9003, 2, 20, strings + 20);
        }
        out.extend(u32b(0));
        out.resize(usize::try_from(strings).expect("offset"), 0);
        for text in [Some(modified), original].into_iter().flatten() {
            out.extend(text.as_bytes());
            out.push(0);
        }
        out
    }

    /// A small JPEG carrying `tiff` in an APP1 Exif segment, as a camera
    /// writes it.
    fn jpeg_with_exif(path: &Path, tiff: &[u8]) {
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut plain)
            .write_image(&[128; 16 * 16 * 3], 16, 16, image::ExtendedColorType::Rgb8)
            .expect("jpeg");
        let payload = [b"Exif\0\0".as_slice(), tiff].concat();
        let length = u16::try_from(payload.len() + 2).expect("segment");
        let mut bytes = plain[..2].to_vec();
        bytes.extend([0xFF, 0xE1]);
        bytes.extend(length.to_be_bytes());
        bytes.extend(payload);
        bytes.extend(&plain[2..]);
        fs::write(path, bytes).expect("write");
    }

    #[test]
    fn a_photo_copied_today_keeps_the_day_the_camera_took_it() {
        let root = std::env::temp_dir().join(format!("frame-host-exif-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("dir");
        let input = root.join("IMG_0001.jpg");
        jpeg_with_exif(
            &input,
            &exif_block(true, "2024:01:02 08:00:00", Some("2019:07:14 12:34:56")),
        );
        let push = prepare(&input, Fit::Crop, &Manifest::default(), false).expect("prepare");
        // 14 July 2019 12:34:56, not the moment the file was written above.
        assert_eq!(push.photos[0].photo.taken, 1_563_107_696);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_capture_date_wins_then_the_save_date_then_nothing_in_either_byte_order() {
        for little in [true, false] {
            let both = exif_block(little, "2024:01:02 08:00:00", Some("2019:07:14 12:34:56"));
            let tiff = Tiff::new(&both).expect("tiff");
            let first = tiff.u32_at(4).expect("first");
            assert_eq!(tiff.date(38, DATE_TIME_ORIGINAL), Some(1_563_107_696));
            assert_eq!(tiff.date(first, DATE_TIME), Some(1_704_182_400));
            let unset = exif_block(little, "0000:00:00 00:00:00", None);
            let tiff = Tiff::new(&unset).expect("tiff");
            assert_eq!(tiff.date(first, DATE_TIME), None);
            assert_eq!(tiff.date(38, DATE_TIME_ORIGINAL), None);
        }
        assert!(Tiff::new(b"II*").is_none());
        assert_eq!(exif_taken(b"not an image"), None);
    }

    #[test]
    fn pushing_a_photo_again_replaces_its_copied_date_with_the_cameras() {
        let root = std::env::temp_dir().join(format!("frame-host-redate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("dir");
        let input = root.join("IMG_0002.jpg");
        jpeg_with_exif(&input, &exif_block(false, "2019:07:14 12:34:56", None));
        let first = prepare(&input, Fit::Crop, &Manifest::default(), false).expect("first");
        let mut old = first.manifest.clone();
        old.photos[0].taken = 1_900_000_000;
        for delete in [false, true] {
            let again = prepare(&input, Fit::Crop, &old, delete).expect("again");
            assert!(again.photos[0].png.is_none(), "the photo is not re-sent");
            assert_eq!(again.manifest.photos[0].taken, 1_563_107_696);
            assert_eq!(again.manifest.photos[0].id, old.photos[0].id);
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn heic_is_explained_before_it_is_decoded() {
        assert!(supported(Path::new("portrait.heic"))
            .expect_err("refuse")
            .contains("convert it to JPEG or PNG"));
    }
}
