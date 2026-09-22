//! Validate and transfer Fieldbook packs, and receive prepared eBird checklists.
use kobo_json::Value;
use serde_json::{json, Value as JsonValue};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFEST: &str = "packs.v1";
const MAX_MANIFEST: usize = 512 * 1024;
const MAX_CHECKLIST: usize = 4 * 1024 * 1024;
const MAX_PHOTO: usize = 2 * 1024 * 1024;
const AVICOMMONS_CATALOG: &str = "https://avicommons.org/2025.json";
const DEVICE_DATA: &str = "/mnt/onboard/.adds/cobalt/data/fieldbook";
const DEVICE_STATE: &str = "/mnt/onboard/.adds/cobalt/state/fieldbook";
const CHECKLIST: &str = "export-checklist.csv";
const USAGE: &str = "usage: kobo fieldbook photos PACK.json --out PACK_DIR [--cache DIR]\n\
                     \x20      kobo fieldbook inspect PACK.json\n\
                     \x20      kobo fieldbook push PACK.json (--sim | --device IP)\n\
                     \x20      kobo fieldbook ls (--sim | --device IP)\n\
                     \x20      kobo fieldbook export (--sim | --device IP) --out FILE.csv\n\
                     Pack manifests are limited to 512 KiB. Export receives the checklist prepared in Fieldbook.";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Summary {
    packs: usize,
    species: usize,
    failures: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Sim,
    Device(String),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("photos") => photos(&arguments[1..]),
        Some("inspect") if arguments.len() == 2 => {
            let input = Path::new(&arguments[1]);
            let summary = if input.is_dir() {
                inspect_directory(input)?
            } else {
                inspect(&bounded(input, MAX_MANIFEST, "pack manifest")?)?
            };
            print_summary(&summary);
            Ok(())
        }
        Some("push") if arguments.len() >= 3 => {
            let input = Path::new(&arguments[1]);
            let target = parse_target(&arguments[2..])?;
            let summary = if input.is_dir() {
                let summary = inspect_directory(input)?;
                publish_directory(target, input)?;
                summary
            } else {
                let bytes = bounded(input, MAX_MANIFEST, "pack manifest")?;
                let summary = inspect(&bytes)?;
                publish(target, &bytes)?;
                summary
            };
            print!("Fieldbook pack ready: ");
            print_summary(&summary);
            Ok(())
        }
        Some("ls") => {
            let target = parse_target(&arguments[1..])?;
            match read_target(&target, MANIFEST, MAX_MANIFEST, true) {
                Ok(bytes) => {
                    print_summary(&inspect(&bytes)?);
                    Ok(())
                }
                Err(message) if message.contains("No such file") => {
                    println!("No packs on this shelf yet.");
                    Ok(())
                }
                Err(message) => Err(message),
            }
        }
        Some("export") => export(&arguments[1..]),
        _ => Err(USAGE.into()),
    }
}

#[derive(Clone, Debug)]
struct PhotoRecord {
    code: String,
    key: String,
    creator: String,
    license: String,
}

#[allow(clippy::too_many_lines)]
fn photos(arguments: &[String]) -> Result<(), String> {
    let out_at = arguments.iter().position(|v| v == "--out").ok_or(USAGE)?;
    let output = arguments.get(out_at + 1).ok_or(USAGE)?;
    let cache_at = arguments.iter().position(|v| v == "--cache");
    let cache = cache_at
        .and_then(|at| arguments.get(at + 1))
        .map_or_else(default_photo_cache, PathBuf::from);
    let positional: Vec<&String> = arguments
        .iter()
        .enumerate()
        .filter(|(at, _)| {
            *at != out_at
                && *at != out_at + 1
                && cache_at.is_none_or(|cache_at| *at != cache_at && *at != cache_at + 1)
        })
        .map(|(_, value)| value)
        .collect();
    if positional.len() != 1 {
        return Err(USAGE.into());
    }
    let input = bounded(Path::new(positional[0]), MAX_MANIFEST, "pack manifest")?;
    let mut shelf: JsonValue = serde_json::from_slice(&input)
        .map_err(|error| format!("invalid Fieldbook pack: {error}"))?;
    validate_json_shelf(&shelf)?;
    let output = Path::new(output);
    if output.exists() {
        return Err(format!("{} already exists", output.display()));
    }
    fs::create_dir_all(&cache).map_err(|error| format!("create {}: {error}", cache.display()))?;
    let catalog_path = cache.join("avicommons-2025.json");
    if !catalog_path.exists() {
        download(AVICOMMONS_CATALOG, &catalog_path, 4 * 1024 * 1024)?;
    }
    let catalog_bytes = bounded(&catalog_path, 4 * 1024 * 1024, "Avicommons catalog")?;
    let records: Vec<JsonValue> = serde_json::from_slice(&catalog_bytes)
        .map_err(|error| format!("invalid Avicommons catalog: {error}"))?;
    let mut by_code = HashMap::new();
    let mut by_scientific = HashMap::new();
    for value in records {
        let Some(record) = photo_record(&value) else {
            continue;
        };
        if accepted_license(&record.license).is_none() {
            continue;
        }
        by_scientific.insert(
            value
                .get("sciName")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_lowercase(),
            record.clone(),
        );
        by_code.insert(record.code.to_lowercase(), record);
    }
    let stage = output.with_extension(format!("fieldbook-writing-{}", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(stage.join("photos")).map_err(|error| error.to_string())?;
    let mut attributions = Vec::new();
    let mut attributed = HashSet::new();
    let mut attached = 0usize;
    let mut unavailable = 0usize;
    let packs = shelf
        .get_mut("packs")
        .and_then(JsonValue::as_array_mut)
        .ok_or("the Fieldbook manifest needs packs")?;
    for pack in packs {
        let species = pack
            .get_mut("species")
            .and_then(JsonValue::as_array_mut)
            .ok_or("a Fieldbook pack needs species")?;
        for bird in species {
            let code = bird
                .get("code")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_lowercase();
            let scientific = bird
                .get("scientific")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_lowercase();
            let record = by_code
                .get(&code)
                .or_else(|| by_scientific.get(&scientific));
            let Some(record) = record else {
                unavailable += 1;
                continue;
            };
            let image_url = format!(
                "https://static.avicommons.org/{}-{}-320.jpg",
                record.code, record.key
            );
            let source_cache = cache.join(format!("{}-{}-320.jpg", record.code, record.key));
            if !source_cache.exists() {
                if let Err(error) = download(&image_url, &source_cache, MAX_PHOTO) {
                    eprintln!("{}: {error}", record.code);
                    unavailable += 1;
                    continue;
                }
            }
            let bytes = bounded(&source_cache, MAX_PHOTO, "photo")?;
            if !is_jpeg(&bytes) {
                let _ = fs::remove_file(&source_cache);
                return Err(format!(
                    "Avicommons returned a non-JPEG for {}",
                    record.code
                ));
            }
            let digest = kobo_net::sha256::hex_digest(&bytes);
            let asset = format!("photos/{digest}.jpg");
            let destination = stage.join(&asset);
            if !destination.exists() {
                fs::copy(&source_cache, &destination)
                    .map_err(|error| format!("copy {}: {error}", destination.display()))?;
            }
            let (license, license_url) = accepted_license(&record.license).expect("filtered above");
            bird.as_object_mut()
                .ok_or("a Fieldbook species must be an object")?
                .insert(
                    "photo".to_owned(),
                    json!({"asset": asset, "attribution": digest}),
                );
            if attributed.insert(digest.clone()) {
                attributions.push(json!({
                    "id": digest,
                    "asset": asset,
                    "sha256": digest,
                    "bytes": bytes.len(),
                    "source": "Avicommons",
                    "source_url": format!("https://avicommons.org/species/{}", record.code),
                    "image_url": image_url,
                    "creator": record.creator,
                    "license": license,
                    "license_url": license_url,
                }));
            }
            attached += 1;
        }
    }
    shelf["version"] = json!("2");
    let manifest = serde_json::to_vec_pretty(&shelf).map_err(|error| error.to_string())?;
    fs::write(stage.join(MANIFEST), &manifest).map_err(|error| error.to_string())?;
    let attribution = json!({
        "format": "fieldbook-attribution",
        "version": "1",
        "photos": attributions,
    });
    fs::write(
        stage.join("attribution.json"),
        serde_json::to_vec_pretty(&attribution).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&stage, output).map_err(|error| {
        let _ = fs::remove_dir_all(&stage);
        format!("publish {}: {error}", output.display())
    })?;
    println!(
        "Fieldbook photo pack: {attached} photo(s), {unavailable} without an eligible Avicommons photo\nSaved {}",
        output.display()
    );
    Ok(())
}

fn default_photo_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cobalt/fieldbook")
}

fn download(url: &str, path: &Path, max: usize) -> Result<(), String> {
    let partial = path.with_extension(format!("download-{}", std::process::id()));
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--fail",
            "--max-filesize",
        ])
        .arg(max.to_string())
        .arg("--output")
        .arg(&partial)
        .arg(url)
        .output()
        .map_err(|error| format!("start curl: {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "download {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata = fs::metadata(&partial).map_err(|error| error.to_string())?;
    if metadata.len() == 0 || metadata.len() > max as u64 {
        let _ = fs::remove_file(&partial);
        return Err(format!("download {url}: invalid size {}", metadata.len()));
    }
    fs::rename(&partial, path).map_err(|error| format!("cache {}: {error}", path.display()))
}

fn photo_record(value: &JsonValue) -> Option<PhotoRecord> {
    Some(PhotoRecord {
        code: value.get("code")?.as_str()?.to_owned(),
        key: value.get("key")?.as_str()?.to_owned(),
        creator: value.get("by")?.as_str()?.to_owned(),
        license: value.get("license")?.as_str()?.to_owned(),
    })
}

fn accepted_license(value: &str) -> Option<(String, String)> {
    let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    let (short, family, default_version) = if normalized == "cc0" || normalized.starts_with("cc0-")
    {
        ("CC0", "publicdomain/zero", "1.0")
    } else if normalized == "cc-by-sa" || normalized.starts_with("cc-by-sa-") {
        ("CC BY-SA", "licenses/by-sa", "4.0")
    } else if normalized == "cc-by"
        || normalized
            .strip_prefix("cc-by-")
            .and_then(|tail| tail.chars().next())
            .is_some_and(|ch| ch.is_ascii_digit())
    {
        ("CC BY", "licenses/by", "4.0")
    } else {
        return None;
    };
    let version = normalized
        .split('-')
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .unwrap_or(default_version);
    Some((
        format!("{short} {version}"),
        format!("https://creativecommons.org/{family}/{version}/"),
    ))
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff]) && bytes.ends_with(&[0xff, 0xd9])
}

fn validate_json_shelf(root: &JsonValue) -> Result<(), String> {
    if root.get("format").and_then(JsonValue::as_str) != Some("fieldbook-shelf")
        || root.get("version").and_then(JsonValue::as_str) != Some("1")
    {
        return Err("photo import expects a Fieldbook shelf version 1 manifest".into());
    }
    if root.get("packs").and_then(JsonValue::as_array).is_none() {
        return Err("the Fieldbook manifest needs packs".into());
    }
    Ok(())
}

fn parse_target(arguments: &[String]) -> Result<Target, String> {
    match arguments {
        [flag] if flag == "--sim" => Ok(Target::Sim),
        [flag, host] if super::is_device_flag(flag) && super::valid_device_host(host) => {
            Ok(Target::Device(host.clone()))
        }
        _ => Err(USAGE.into()),
    }
}
fn bounded(path: &Path, max: usize, label: &str) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > max as u64 {
        return Err(format!("the {label} exceeds the {} KiB limit", max / 1024));
    }
    fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}
fn text<'a>(value: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{what} needs {key}"))
}
fn inspect(bytes: &[u8]) -> Result<Summary, String> {
    if bytes.len() > MAX_MANIFEST {
        return Err("the pack manifest exceeds the 512 KiB limit".into());
    }
    let root =
        kobo_json::parse(std::str::from_utf8(bytes).map_err(|_| "the pack manifest is not UTF-8")?)
            .map_err(|e| format!("invalid Fieldbook pack: {e}"))?;
    let version = text(&root, "version", "manifest")?;
    if text(&root, "format", "manifest")? != "fieldbook-shelf" || (version != "1" && version != "2")
    {
        return Err("this is not a Fieldbook shelf version 1 or 2 manifest".into());
    }
    let packs = root
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("the Fieldbook manifest needs packs")?;
    let failures = root
        .get("failures")
        .and_then(Value::as_array)
        .ok_or("the Fieldbook manifest needs failures")?;
    let mut species = 0;
    for pack in packs {
        for key in ["id", "title", "region", "issued"] {
            let _ = text(pack, key, "a Fieldbook pack")?;
        }
        let entries = pack
            .get("species")
            .and_then(Value::as_array)
            .ok_or("a Fieldbook pack needs species")?;
        for bird in entries {
            for key in ["code", "common", "scientific"] {
                let _ = text(bird, key, "a Fieldbook species")?;
            }
        }
        species += entries.len();
    }
    for failure in failures {
        let _ = text(failure, "input", "a Fieldbook failure")?;
        let _ = text(failure, "reason", "a Fieldbook failure")?;
    }
    Ok(Summary {
        packs: packs.len(),
        species,
        failures: failures.len(),
    })
}

fn inspect_directory(root: &Path) -> Result<Summary, String> {
    let manifest_bytes = bounded(&root.join(MANIFEST), MAX_MANIFEST, "pack manifest")?;
    let shelf: JsonValue = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid Fieldbook pack: {error}"))?;
    if shelf.get("format").and_then(JsonValue::as_str) != Some("fieldbook-shelf")
        || shelf.get("version").and_then(JsonValue::as_str) != Some("2")
    {
        return Err("this is not a Fieldbook shelf version 2 directory".into());
    }
    let attribution_bytes = bounded(
        &root.join("attribution.json"),
        MAX_MANIFEST,
        "photo attribution manifest",
    )?;
    let attribution: JsonValue = serde_json::from_slice(&attribution_bytes)
        .map_err(|error| format!("invalid Fieldbook attribution manifest: {error}"))?;
    if attribution.get("format").and_then(JsonValue::as_str) != Some("fieldbook-attribution")
        || attribution.get("version").and_then(JsonValue::as_str) != Some("1")
    {
        return Err("the photo attribution manifest must be version 1".into());
    }
    let mut credits = HashMap::new();
    for credit in attribution
        .get("photos")
        .and_then(JsonValue::as_array)
        .ok_or("the photo attribution manifest needs photos")?
    {
        let id = required_json_text(credit, "id", "a photo attribution")?;
        let asset = required_json_text(credit, "asset", "a photo attribution")?;
        let digest = required_json_text(credit, "sha256", "a photo attribution")?;
        let expected = credit
            .get("bytes")
            .and_then(JsonValue::as_u64)
            .ok_or("a photo attribution needs bytes")?;
        for key in ["source", "source_url", "creator", "license", "license_url"] {
            let _ = required_json_text(credit, key, "a photo attribution")?;
        }
        validate_asset_name(asset, digest)?;
        let bytes = bounded(&root.join(asset), MAX_PHOTO, "photo")?;
        if bytes.len() as u64 != expected
            || kobo_net::sha256::hex_digest(&bytes) != digest
            || !is_jpeg(&bytes)
        {
            return Err(format!(
                "photo {asset} does not match its attribution record"
            ));
        }
        if credits.insert(id.to_owned(), asset.to_owned()).is_some() {
            return Err(format!("duplicate photo attribution id {id}"));
        }
    }
    let packs = shelf
        .get("packs")
        .and_then(JsonValue::as_array)
        .ok_or("the Fieldbook manifest needs packs")?;
    let failures = shelf
        .get("failures")
        .and_then(JsonValue::as_array)
        .ok_or("the Fieldbook manifest needs failures")?;
    let mut species_count = 0;
    let mut used_credits = HashSet::new();
    for pack in packs {
        for key in ["id", "title", "region", "issued"] {
            let _ = required_json_text(pack, key, "a Fieldbook pack")?;
        }
        let species = pack
            .get("species")
            .and_then(JsonValue::as_array)
            .ok_or("a Fieldbook pack needs species")?;
        species_count += species.len();
        for bird in species {
            for key in ["code", "common", "scientific"] {
                let _ = required_json_text(bird, key, "a Fieldbook species")?;
            }
            if let Some(photo) = bird.get("photo") {
                let asset = required_json_text(photo, "asset", "a species photo")?;
                let credit = required_json_text(photo, "attribution", "a species photo")?;
                if credits.get(credit).map(String::as_str) != Some(asset) {
                    return Err(format!(
                        "species photo {asset} has no matching attribution record {credit}"
                    ));
                }
                used_credits.insert(credit.to_owned());
            }
        }
    }
    if used_credits.len() != credits.len() {
        return Err("the attribution manifest contains an unreferenced photo".into());
    }
    for failure in failures {
        let _ = required_json_text(failure, "input", "a Fieldbook failure")?;
        let _ = required_json_text(failure, "reason", "a Fieldbook failure")?;
    }
    Ok(Summary {
        packs: packs.len(),
        species: species_count,
        failures: failures.len(),
    })
}

fn required_json_text<'a>(value: &'a JsonValue, key: &str, what: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("{what} needs {key}"))
}

fn validate_asset_name(asset: &str, digest: &str) -> Result<(), String> {
    let expected = format!("photos/{digest}.jpg");
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || asset != expected
    {
        return Err(format!(
            "unsafe or non-content-addressed photo asset {asset}"
        ));
    }
    Ok(())
}

fn print_summary(s: &Summary) {
    println!(
        "{} pack(s) · {} species · {} import failure(s)",
        s.packs, s.species, s.failures
    );
}
fn sim_data() -> PathBuf {
    kobo_sim::simulated_data_root("fieldbook")
}
fn publish(target: Target, bytes: &[u8]) -> Result<(), String> {
    match target {
        Target::Sim => atomic(&sim_data().join(MANIFEST), bytes),
        Target::Device(host) => remote_write(&host, DEVICE_DATA, MANIFEST, bytes),
    }
}

fn publish_directory(target: Target, source: &Path) -> Result<(), String> {
    match target {
        Target::Sim => copy_directory_atomic(source, &sim_data()),
        Target::Device(host) => remote_directory(&host, source),
    }
}

fn copy_directory_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination.parent().ok_or("destination has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let stage = parent.join(format!(".fieldbook-pack-{}.writing", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|error| error.to_string())?;
    }
    copy_tree(source, &stage)?;
    flatten_photos(&stage)?;
    let backup = parent.join(format!(".fieldbook-pack-{}.old", std::process::id()));
    if destination.exists() {
        fs::rename(destination, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&stage, destination) {
        if backup.exists() {
            let _ = fs::rename(&backup, destination);
        }
        return Err(error.to_string());
    }
    if backup.exists() {
        fs::remove_dir_all(backup).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The reader's shelf answers flat names of at most 64 characters, so a
/// pack's photos/<digest>.jpg assets publish as root-level <digest> files:
/// the full content address fits the key limit, the extension does not.
/// The manifest keeps the photos/ path; the reader derives the same key
/// from the asset's file stem.
fn flatten_photos(stage: &Path) -> Result<(), String> {
    let photos = stage.join("photos");
    if !photos.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&photos).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("the pack photos directory holds more than files".into());
        }
        let name = entry.file_name();
        let name = name.to_str().ok_or("photo file name is not UTF-8")?;
        // The reader derives the shelf key from the file stem, whatever
        // extension the photo carries.
        let name = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        fs::rename(entry.path(), stage.join(name)).map_err(|error| error.to_string())?;
    }
    fs::remove_dir(&photos).map_err(|error| error.to_string())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "pack contains a symlink: {}",
                entry.path().display()
            ));
        }
        let output = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &output)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), output).map_err(|error| error.to_string())?;
        } else {
            return Err(format!(
                "pack contains a non-file: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn remote_directory(host: &str, source: &Path) -> Result<(), String> {
    let mut files = vec![source.join(MANIFEST), source.join("attribution.json")];
    let photos = source.join("photos");
    for entry in fs::read_dir(&photos).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        files.push(entry.path());
    }
    let stage = format!("{DEVICE_DATA}.{}.writing", std::process::id());
    let mut script = format!(
        "set -eu\nstage='{stage}'\nrm -rf \"$stage\"\nmkdir -p \"$stage\"\ntrap 'rm -rf \"$stage\"' EXIT HUP INT TERM\n"
    );
    for path in files {
        let relative = path
            .strip_prefix(source)
            .map_err(|_| "photo path escaped pack")?;
        let relative = relative.to_str().ok_or("photo path is not UTF-8")?;
        if relative.contains('\'') || relative.contains('\n') || relative.contains("..") {
            return Err("unsafe photo pack filename".into());
        }
        // The reader's shelf answers flat names of at most 64 characters;
        // photos/<digest>.jpg stages as <digest> and the reader derives
        // the same key from the asset's file stem.
        let staged = relative.strip_prefix("photos/").map_or(relative, |name| {
            name.rsplit_once('.').map_or(name, |(stem, _)| stem)
        });
        let bytes = bounded(&path, MAX_PHOTO, "photo pack file")?;
        let encoded = super::base64_encode(&bytes);
        let digest = kobo_net::sha256::hex_digest(&bytes);
        script.push_str(&format!(
            "base64 -d > \"$stage/{staged}\" <<'FIELD_BOOK_FILE'\n{encoded}\nFIELD_BOOK_FILE\nset -- $(sha256sum \"$stage/{staged}\"); test \"$1\" = '{digest}'\n"
        ));
    }
    script.push_str(&format!(
        "old='{DEVICE_DATA}.old'\nrm -rf \"$old\"\nif test -e '{DEVICE_DATA}'; then mv '{DEVICE_DATA}' \"$old\"; fi\nmv \"$stage\" '{DEVICE_DATA}'\nrm -rf \"$old\"\ntrap - EXIT HUP INT TERM\nsync\n"
    ));
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if output.status.success() {
        Ok(())
    } else {
        Err("the reader refused the Fieldbook photo pack".into())
    }
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("destination has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let partial = parent.join(format!(".{MANIFEST}.{}.writing", std::process::id()));
    let result = (|| {
        fs::write(&partial, bytes).map_err(|e| e.to_string())?;
        fs::rename(&partial, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}
fn remote_write(host: &str, root: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let encoded = super::base64_encode(bytes);
    let count = bytes.len();
    let digest = kobo_net::sha256::hex_digest(bytes);
    let script = format!(
        "set -eu\nroot='{root}'\nmkdir -p \"$root\"\npartial=\"$root/.{name}.$$.writing\"\ntrap 'rm -f \"$partial\"' EXIT HUP INT TERM\nbase64 -d > \"$partial\" <<'FIELD_BOOK'\n{encoded}\nFIELD_BOOK\ntest \"$(wc -c < \"$partial\")\" = '{count}'\nset -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{name}\"\nsync\n"
    );
    let out = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if out.status.success() {
        Ok(())
    } else {
        Err("the reader refused the Fieldbook transfer".into())
    }
}
fn read_target(target: &Target, name: &str, max: usize, data: bool) -> Result<Vec<u8>, String> {
    match target {
        Target::Sim => {
            let root = if data {
                sim_data()
            } else {
                std::env::temp_dir().join("cobalt-sim-state/fieldbook")
            };
            bounded(&root.join(name), max, name)
        }
        Target::Device(host) => {
            let root = if data { DEVICE_DATA } else { DEVICE_STATE };
            let script = format!(
                "set -eu\ntest -f '{root}/{name}' && test ! -L '{root}/{name}'\nhead -c {} '{root}/{name}'\n",
                max + 1
            );
            let out = super::run_remote_shell(
                &format!("root@{host}"),
                &script,
                super::REMOTE_COMMAND_TIMEOUT,
            )
            .map_err(super::unreachable_device)?;
            if !out.status.success() || out.stdout.len() > max {
                return Err(format!("no valid {name} is available"));
            }
            Ok(out.stdout)
        }
    }
}
fn export(args: &[String]) -> Result<(), String> {
    let out_at = args.iter().position(|v| v == "--out").ok_or(USAGE)?;
    let output = args.get(out_at + 1).ok_or(USAGE)?;
    let mut target_args = args.to_vec();
    target_args.drain(out_at..=out_at + 1);
    let target = parse_target(&target_args)?;
    let bytes = read_target(&target, CHECKLIST, MAX_CHECKLIST, false)?;
    if bytes.is_empty() || !bytes.starts_with(b",,") {
        return Err("the prepared checklist is not eBird Checklist Format CSV".into());
    }
    let output = Path::new(output);
    if output.exists() {
        return Err(format!(
            "{} already exists; choose a new filename",
            output.display()
        ));
    }
    atomic_output(output, &bytes)?;
    println!(
        "Saved {}\nThe original remains in Fieldbook.",
        output.display()
    );
    Ok(())
}
fn atomic_output(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let part = parent.join(format!(".fieldbook-export-{}.part", std::process::id()));
    fs::write(&part, bytes).map_err(|e| e.to_string())?;
    let r = fs::hard_link(&part, path).map_err(|e| format!("save checklist: {e}"));
    let _ = fs::remove_file(part);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Vec<u8> {
        br#"{"format":"fieldbook-shelf","version":"1","packs":[{"id":"cp","title":"Central Park","region":"US-NY","issued":"2026-09-18","species":[{"code":"AMRO","common":"American Robin","scientific":"Turdus migratorius"}]}],"failures":[]}"#.to_vec()
    }
    #[test]
    fn validates_contract() {
        assert_eq!(
            inspect(&sample()).unwrap(),
            Summary {
                packs: 1,
                species: 1,
                failures: 0
            }
        );
    }
    #[test]
    fn photo_license_filter_is_fail_closed() {
        assert!(accepted_license("CC0 1.0").is_some());
        assert!(accepted_license("CC BY 2.0").is_some());
        assert!(accepted_license("CC BY-SA 4.0").is_some());
        assert!(accepted_license("CC BY-NC 4.0").is_none());
        assert!(accepted_license("CC BY-ND 4.0").is_none());
        assert!(accepted_license("unknown").is_none());
    }
    #[test]
    fn jpeg_check_rejects_text_and_truncation() {
        assert!(is_jpeg(&[0xff, 0xd8, 0xff, 0xdb, 0xff, 0xd9]));
        assert!(!is_jpeg(b"not a photo"));
        assert!(!is_jpeg(&[0xff, 0xd8, 0xff]));
    }
    #[test]
    fn pack_directory_publishes_photos_flat() {
        let unique = format!("fieldbook-flat-{}", std::process::id());
        let root = std::env::temp_dir().join(&unique);
        let source = root.join("pack");
        let photos = source.join("photos");
        fs::create_dir_all(&photos).unwrap();
        let digest = "a".repeat(64);
        fs::write(source.join(MANIFEST), sample()).unwrap();
        fs::write(
            source.join("attribution.json"),
            br#"{"format":"fieldbook-attribution","version":"1","photos":[]}"#,
        )
        .unwrap();
        fs::write(photos.join(format!("{digest}.jpg")), b"jpeg-bytes").unwrap();
        let destination = root.join("sim-data");
        copy_directory_atomic(&source, &destination).unwrap();
        assert!(destination.join(MANIFEST).is_file());
        assert!(destination.join(&digest).is_file());
        assert!(!destination.join("photos").exists());
        let _ = fs::remove_dir_all(&root);
    }
    #[test]
    fn rejects_wrong_schema_and_incomplete_species() {
        assert!(inspect(b"{}").is_err());
        assert!(inspect(
            br#"{"format":"fieldbook-shelf","version":"1","packs":[],"failures":null}"#
        )
        .is_err());
    }
}
