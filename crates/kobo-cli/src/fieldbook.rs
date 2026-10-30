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
const ATTRIBUTION: &str = "attribution.json";
const USAGE: &str = concat!(
    "usage: kobo fieldbook new SPECIES.csv --title TITLE [--region REGION] --out PACK.json\n\
                     \x20      kobo fieldbook photos PACK.json --out PACK_DIR [--cache DIR]\n\
                     \x20      kobo fieldbook inspect PACK.json\n\
                     \x20      kobo fieldbook push PACK.json ",
    crate::targets::target_choice!(),
    " [--replace]\n\
                     \x20      kobo fieldbook ls ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo fieldbook export ",
    crate::targets::target_choice!(),
    " --out FILE.csv\n\
                     new makes a pack from a CSV list of birds, such as eBird's My eBird data download\n\
                     or a region's species list, with Common Name and Scientific Name columns.\n\
                     A push adds its packs to the ones on the reader, replacing a pack with the same id.\n\
                     --replace leaves only the pushed packs. Pack manifests are limited to 512 KiB.\n\
                     Export receives the checklist prepared in Fieldbook.\n",
    crate::targets::saved_reader_note!(),
);

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
        Some("new") => new_pack(&arguments[1..]),
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
            let mut rest = arguments[2..].to_vec();
            let replace = take_flag(&mut rest, "--replace");
            let target = parse_target(&rest)?;
            let summary = if input.is_dir() {
                let summary = inspect_directory(input)?;
                if replace {
                    publish_directory(target.clone(), input)?;
                } else {
                    add_directory(&target, input)?;
                }
                summary
            } else {
                let bytes = bounded(input, MAX_MANIFEST, "pack manifest")?;
                let summary = inspect(&bytes)?;
                if replace {
                    publish(target.clone(), &bytes)?;
                } else {
                    let existing = read_existing(&target, MANIFEST)?;
                    publish(target.clone(), &merge_shelves(existing.as_deref(), &bytes)?)?;
                }
                summary
            };
            print!("Sent: ");
            print_summary(&summary);
            print_on_reader(&target)
        }
        Some("ls") => {
            let target = parse_target(&arguments[1..])?;
            print_on_reader(&target)
        }
        Some("export") => export(&arguments[1..]),
        _ => Err(USAGE.into()),
    }
}

/// Makes a pack from a list of birds the owner already has: eBird's "My
/// eBird data" download, a region's species list saved as CSV, or a sheet of
/// their own. Columns are found by their header, so any of these work
/// without editing; a list with no codes gets four-letter codes made the way
/// banding codes are, one per bird.
fn new_pack(arguments: &[String]) -> Result<(), String> {
    let mut input = None;
    let mut title = None;
    let mut region = None;
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        let value = || arguments.get(index + 1).ok_or_else(|| USAGE.to_owned());
        match arguments[index].as_str() {
            "--title" => title = Some(value()?.clone()),
            "--region" => region = Some(value()?.clone()),
            "--out" => output = Some(value()?.clone()),
            other if !other.starts_with('-') && input.is_none() => {
                input = Some(other.to_owned());
                index += 1;
                continue;
            }
            _ => return Err(USAGE.into()),
        }
        index += 2;
    }
    let (Some(input), Some(title), Some(output)) = (input, title, output) else {
        return Err(USAGE.into());
    };
    let output = Path::new(&output);
    if output.exists() {
        return Err(format!("{} already exists", output.display()));
    }
    let csv = bounded(Path::new(&input), 16 * 1024 * 1024, "species list")?;
    let csv = String::from_utf8(csv).map_err(|_| format!("{input} is not UTF-8 text"))?;
    let species = species_from_csv(&csv)?;
    let pack = pack_json(
        &title,
        region.as_deref().unwrap_or(&title),
        &species,
        &today(),
    );
    let bytes = serde_json::to_vec_pretty(&pack).map_err(|error| error.to_string())?;
    inspect(&bytes)?;
    fs::write(output, &bytes).map_err(|error| format!("write {}: {error}", output.display()))?;
    println!(
        "Made {} with {} species. Send it with kobo fieldbook push {}",
        output.display(),
        species.len(),
        output.display()
    );
    Ok(())
}

fn today() -> String {
    use kobo_policy::clock::Clock as _;
    kobo_policy::clock::SystemClock::new(0)
        .ok()
        .and_then(|clock| clock.now().ok())
        .and_then(kobo_policy::clock::Snapshot::date)
        .map_or_else(|| "1970-01-01".to_owned(), |date| date.to_string())
}

fn pack_json(
    title: &str,
    region: &str,
    species: &[(String, String, String)],
    issued: &str,
) -> JsonValue {
    let mut id = String::new();
    for character in title.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_end_matches('-');
    json!({
        "format": "fieldbook-shelf",
        "version": "1",
        "packs": [{
            "id": if id.is_empty() { "pack" } else { id },
            "title": title,
            "region": region,
            "issued": issued,
            "species": species
                .iter()
                .map(|(code, common, scientific)| json!({
                    "code": code,
                    "common": common,
                    "scientific": scientific,
                }))
                .collect::<Vec<_>>(),
        }],
        "failures": [],
    })
}

/// One `(code, common, scientific)` per bird, in list order, each bird once.
fn species_from_csv(text: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut rows = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(csv_fields);
    let header = rows
        .next()
        .ok_or("the species list is empty")?
        .into_iter()
        .map(|field| field.trim().trim_start_matches('\u{feff}').to_lowercase())
        .collect::<Vec<_>>();
    let column = |names: &[&str]| {
        header
            .iter()
            .position(|field| names.contains(&field.as_str()))
    };
    let common = column(&[
        "common name",
        "common",
        "primary_com_name",
        "english name",
        "english_name",
    ])
    .ok_or("the species list needs a Common Name column")?;
    let scientific = column(&[
        "scientific name",
        "scientific",
        "sci_name",
        "scientific_name",
    ])
    .ok_or("the species list needs a Scientific Name column")?;
    let code = column(&[
        "code",
        "banding code",
        "banding_codes",
        "alpha code",
        "species_code",
        "species code",
    ]);
    let category = column(&["category"]);
    let mut species: Vec<(String, String, String)> = Vec::new();
    for row in rows {
        let field = |index: usize| row.get(index).map_or("", |value| value.trim());
        // A taxonomy lists hybrids, slashes and "sp." entries as well as
        // species; only the species belong on a tally.
        if category.is_some_and(|index| !field(index).is_empty() && field(index) != "species") {
            continue;
        }
        let (name, latin) = (field(common), field(scientific));
        if name.is_empty() || latin.is_empty() {
            continue;
        }
        if species
            .iter()
            .any(|(_, _, known)| known.eq_ignore_ascii_case(latin))
        {
            continue;
        }
        let given = code
            .map(field)
            .and_then(|value| value.split_whitespace().next())
            .unwrap_or("");
        let mut made = if given.is_empty() {
            alpha_code(name)
        } else {
            given.to_uppercase()
        };
        if species.iter().any(|(known, _, _)| *known == made) {
            let stem = made.clone();
            let mut number = 2;
            while species.iter().any(|(known, _, _)| *known == made) {
                made = format!("{stem}{number}");
                number += 1;
            }
        }
        species.push((made, name.to_owned(), latin.to_owned()));
    }
    if species.is_empty() {
        return Err(
            "the species list names no birds with both a common and a scientific name".into(),
        );
    }
    Ok(species)
}

/// A four-letter code from a common name: four letters of one word, two
/// and two of two, one, one and two of three, the first letter of each of
/// four. These are the banding-code rules before their exceptions.
fn alpha_code(name: &str) -> String {
    let words = name
        .split(|character: char| character.is_whitespace() || character == '-')
        .map(|word| {
            word.chars()
                .filter(char::is_ascii_alphabetic)
                .collect::<String>()
                .to_uppercase()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let take = |word: &String, count: usize| word.chars().take(count).collect::<String>();
    match words.as_slice() {
        [] => "BIRD".to_owned(),
        [one] => take(one, 4),
        [first, second] => take(first, 2) + &take(second, 2),
        [first, second, third] => take(first, 1) + &take(second, 1) + &take(third, 2),
        many => many.iter().take(4).map(|word| take(word, 1)).collect(),
    }
}

/// The fields of one CSV line, with quoted fields and doubled quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match (character, quoted) {
            ('"', true) if characters.peek() == Some(&'"') => {
                field.push('"');
                characters.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => fields.push(std::mem::take(&mut field)),
            _ => field.push(character),
        }
    }
    fields.push(field);
    fields
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

/// Removes `flag` from the arguments, saying whether it was there.
fn take_flag(arguments: &mut Vec<String>, flag: &str) -> bool {
    let before = arguments.len();
    arguments.retain(|argument| argument != flag);
    arguments.len() != before
}

/// What the reader holds after a push, pack by pack, so an owner sees that
/// adding a region kept the others.
fn print_on_reader(target: &Target) -> Result<(), String> {
    let Some(bytes) = read_existing(target, MANIFEST)? else {
        println!("No field packs are on the reader.");
        return Ok(());
    };
    let summary = inspect(&bytes)?;
    let shelf: JsonValue = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Fieldbook pack: {error}"))?;
    let titles = shelf
        .get("packs")
        .and_then(JsonValue::as_array)
        .map(|packs| {
            packs
                .iter()
                .filter_map(|pack| pack.get("title").and_then(JsonValue::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    print!("On the reader: ");
    print_summary(&summary);
    for title in titles {
        println!("  {title}");
    }
    Ok(())
}

/// A file from the reader's Fieldbook shelf, or nothing when it has none.
fn read_existing(target: &Target, name: &str) -> Result<Option<Vec<u8>>, String> {
    match target {
        Target::Sim => {
            let path = sim_data().join(name);
            if path.exists() {
                bounded(&path, MAX_MANIFEST, name).map(Some)
            } else {
                Ok(None)
            }
        }
        Target::Device(host) => {
            let script = format!(
                "set -eu\nif test -f '{DEVICE_DATA}/{name}' && test ! -L '{DEVICE_DATA}/{name}'; then\n\
                 \x20 printf 'present\\n'\n\
                 \x20 head -c {} '{DEVICE_DATA}/{name}'\n\
                 fi\n",
                MAX_MANIFEST + 1
            );
            let out = super::run_remote_shell(
                &format!("root@{host}"),
                &script,
                super::REMOTE_COMMAND_TIMEOUT,
            )
            .map_err(super::unreachable_device)?;
            if !out.status.success() {
                return Err(format!("the reader could not read its {name}"));
            }
            match out.stdout.strip_prefix(b"present\n") {
                Some(bytes) if bytes.len() > MAX_MANIFEST => {
                    Err(format!("the reader's {name} is larger than 512 KiB"))
                }
                Some(bytes) => Ok(Some(bytes.to_vec())),
                None => Ok(None),
            }
        }
    }
}

/// The reader's shelf with the pushed packs added: a pack with an id the
/// reader already has replaces it where it stands, a new one goes last.
/// Import failures the push reports again replace the old report of them.
fn merge_shelves(existing: Option<&[u8]>, incoming: &[u8]) -> Result<Vec<u8>, String> {
    let incoming: JsonValue = serde_json::from_slice(incoming)
        .map_err(|error| format!("invalid Fieldbook pack: {error}"))?;
    let Some(existing) = existing else {
        return serde_json::to_vec_pretty(&incoming).map_err(|error| error.to_string());
    };
    let Ok(existing) = serde_json::from_slice::<JsonValue>(existing) else {
        // A shelf the reader cannot read is no loss to replace.
        return serde_json::to_vec_pretty(&incoming).map_err(|error| error.to_string());
    };
    let list = |root: &JsonValue, key: &str| {
        root.get(key)
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let text = |value: &JsonValue, key: &str| {
        value
            .get(key)
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let mut packs = list(&existing, "packs");
    for pack in list(&incoming, "packs") {
        let id = text(&pack, "id");
        if let Some(slot) = packs.iter_mut().find(|known| text(known, "id") == id) {
            *slot = pack;
        } else {
            packs.push(pack);
        }
    }
    let fresh = list(&incoming, "failures");
    let mut failures = list(&existing, "failures")
        .into_iter()
        .filter(|old| {
            !fresh
                .iter()
                .any(|new| text(new, "input") == text(old, "input"))
        })
        .collect::<Vec<_>>();
    failures.extend(fresh);
    let version = if text(&existing, "version") == "2" || text(&incoming, "version") == "2" {
        "2"
    } else {
        "1"
    };
    let merged = serde_json::to_vec_pretty(&json!({
        "format": "fieldbook-shelf",
        "version": version,
        "packs": packs,
        "failures": failures,
    }))
    .map_err(|error| error.to_string())?;
    if merged.len() > MAX_MANIFEST {
        return Err(
            "the reader's packs and this one together exceed 512 KiB. Push with --replace to \
             keep only the packs you name"
                .into(),
        );
    }
    Ok(merged)
}

/// Photo credits from both sides, one record per photo id.
fn merge_attribution(existing: Option<&[u8]>, incoming: &[u8]) -> Result<Vec<u8>, String> {
    let incoming: JsonValue = serde_json::from_slice(incoming)
        .map_err(|error| format!("invalid Fieldbook attribution manifest: {error}"))?;
    let mut photos = existing
        .and_then(|bytes| serde_json::from_slice::<JsonValue>(bytes).ok())
        .and_then(|root| root.get("photos").and_then(JsonValue::as_array).cloned())
        .unwrap_or_default();
    for photo in incoming
        .get("photos")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let id = photo.get("id").cloned();
        if let Some(slot) = photos
            .iter_mut()
            .find(|known| known.get("id").cloned() == id)
        {
            *slot = photo;
        } else {
            photos.push(photo);
        }
    }
    serde_json::to_vec_pretty(&json!({
        "format": "fieldbook-attribution",
        "version": "1",
        "photos": photos,
    }))
    .map_err(|error| error.to_string())
}

/// Adds a photo pack to the reader's shelf without removing what is there:
/// the photos land under their content address first, then the merged
/// credits, then the merged manifest that refers to them.
fn add_directory(target: &Target, source: &Path) -> Result<(), String> {
    let manifest = merge_shelves(
        read_existing(target, MANIFEST)?.as_deref(),
        &bounded(&source.join(MANIFEST), MAX_MANIFEST, "pack manifest")?,
    )?;
    let attribution = merge_attribution(
        read_existing(target, ATTRIBUTION)?.as_deref(),
        &bounded(
            &source.join(ATTRIBUTION),
            MAX_MANIFEST,
            "photo attribution manifest",
        )?,
    )?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let photos = source.join("photos");
    if photos.exists() {
        for entry in fs::read_dir(&photos).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("photo file name is not UTF-8")?;
            // The reader's shelf key is the photo's file stem.
            let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
            if stem.len() != 64 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("unsafe or non-content-addressed photo {name}"));
            }
            files.push((stem.to_owned(), bounded(&entry.path(), MAX_PHOTO, "photo")?));
        }
    }
    files.push((ATTRIBUTION.to_owned(), attribution));
    files.push((MANIFEST.to_owned(), manifest));
    match target {
        Target::Sim => {
            for (name, bytes) in &files {
                atomic(&sim_data().join(name), bytes)?;
            }
            Ok(())
        }
        Target::Device(host) => {
            let mut script = format!("set -eu\nroot='{DEVICE_DATA}'\nmkdir -p \"$root\"\n");
            let mut total = 0;
            for (name, bytes) in &files {
                total += bytes.len();
                let digest = kobo_net::sha256::hex_digest(bytes);
                script.push_str(&format!(
                    "partial=\"$root/.{name}.$$.writing\"\n\
                     base64 -d > \"$partial\" <<'FIELD_BOOK_FILE'\n{}\nFIELD_BOOK_FILE\n\
                     set -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\n\
                     chmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{name}\"\n",
                    super::base64_encode(bytes)
                ));
            }
            script.push_str("sync\n");
            let timeout = super::REMOTE_COMMAND_TIMEOUT
                + std::time::Duration::from_secs(u64::try_from(total / 200_000).unwrap_or(0));
            let output = super::run_remote_shell(&format!("root@{host}"), &script, timeout)
                .map_err(super::unreachable_device)?;
            if output.status.success() {
                Ok(())
            } else {
                Err("the reader refused the Fieldbook photo pack".into())
            }
        }
    }
}

fn print_summary(s: &Summary) {
    let plural = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    let mut line = format!(
        "{}, {} species",
        plural(s.packs, "pack", "packs"),
        s.species
    );
    if s.failures > 0 {
        line.push_str(&format!(
            ", {}",
            plural(
                s.failures,
                "pack that could not be read",
                "packs that could not be read"
            )
        ));
    }
    println!("{line}.");
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
                kobo_sim::simulated_state_root("fieldbook")
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
    fn a_pack_is_made_from_an_ebird_download_with_codes_made_for_it() {
        let csv = "Submission ID,Common Name,Scientific Name,Count\n\
                   S1,American Robin,Turdus migratorius,2\n\
                   S1,\"Black-and-white Warbler\",Mniotilta varia,1\n\
                   S2,American Robin,Turdus migratorius,4\n\
                   S2,American Redstart,Setophaga ruticilla,1\n";
        let species = species_from_csv(csv).unwrap();
        assert_eq!(
            species,
            vec![
                (
                    "AMRO".to_owned(),
                    "American Robin".to_owned(),
                    "Turdus migratorius".to_owned()
                ),
                (
                    "BAWW".to_owned(),
                    "Black-and-white Warbler".to_owned(),
                    "Mniotilta varia".to_owned()
                ),
                (
                    "AMRE".to_owned(),
                    "American Redstart".to_owned(),
                    "Setophaga ruticilla".to_owned()
                ),
            ]
        );
        let pack = serde_json::to_vec(&pack_json("Central Park", "US-NY", &species, "2026-10-28"))
            .unwrap();
        assert_eq!(inspect(&pack).unwrap().species, 3);
    }

    #[test]
    fn a_taxonomy_keeps_its_own_codes_and_only_its_species() {
        let csv = "SPECIES_CODE,PRIMARY_COM_NAME,SCI_NAME,CATEGORY\n\
                   amerob,American Robin,Turdus migratorius,species\n\
                   thrush1,thrush sp.,Turdidae sp.,spuh\n";
        let species = species_from_csv(csv).unwrap();
        assert_eq!(species.len(), 1);
        assert_eq!(species[0].0, "AMEROB");
        assert!(species_from_csv("Name,Count\nRobin,2\n").is_err());
    }

    #[test]
    fn a_push_adds_packs_and_replaces_one_with_the_same_id() {
        let existing = br#"{"format":"fieldbook-shelf","version":"1","packs":[{"id":"cp","title":"Central Park","region":"US-NY","issued":"2026-09-18","species":[]},{"id":"sf","title":"San Francisco","region":"US-CA","issued":"2026-09-01","species":[]}],"failures":[{"input":"old.csv","reason":"x"}]}"#;
        let incoming = br#"{"format":"fieldbook-shelf","version":"1","packs":[{"id":"cp","title":"Central Park 2027","region":"US-NY","issued":"2027-01-01","species":[]},{"id":"pr","title":"Prospect Park","region":"US-NY","issued":"2026-10-01","species":[]}],"failures":[]}"#;
        let merged: JsonValue =
            serde_json::from_slice(&merge_shelves(Some(existing), incoming).unwrap()).unwrap();
        let titles = merged["packs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pack| pack["title"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            ["Central Park 2027", "San Francisco", "Prospect Park"]
        );
        assert_eq!(merged["failures"].as_array().unwrap().len(), 1);
        let fresh: JsonValue =
            serde_json::from_slice(&merge_shelves(None, incoming).unwrap()).unwrap();
        assert_eq!(fresh["packs"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn photo_credits_from_two_pushes_are_kept_together() {
        let first = br#"{"format":"fieldbook-attribution","version":"1","photos":[{"id":"a","creator":"A"}]}"#;
        let second = br#"{"format":"fieldbook-attribution","version":"1","photos":[{"id":"b","creator":"B"},{"id":"a","creator":"A2"}]}"#;
        let merged: JsonValue =
            serde_json::from_slice(&merge_attribution(Some(first), second).unwrap()).unwrap();
        let creators = merged["photos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|photo| photo["creator"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(creators, ["A2", "B"]);
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
