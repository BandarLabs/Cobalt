//! Owner-attended packing of an Obsidian Web Clipper folder onto the
//! Clippings app's shelf: real reader storage (`/mnt/onboard/.adds/cobalt/data`),
//! not the small per-app key-value store. Mirrors `frame.rs`'s manifest +
//! per-item shelf blob shape and its incremental diffing, so a re-push after
//! a handful of edits does not retransfer the whole vault.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kobo_json::{ObjectBuilder, Value};

const USAGE: &str = "usage: kobo clippings push DIR (--device IP | --sim)";
const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/clippings";
const MANIFEST: &str = "manifest.v1";
const MAX_MANIFEST: usize = 4 * 1024 * 1024;
/// Generous, not tight: the reader has real storage, and this exists only so
/// a runaway push (a vault accidentally pointed at something enormous) fails
/// with a clear message instead of silently filling the card.
const MAX_CAPACITY: usize = 200 * 1024 * 1024;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, PartialEq)]
struct NoteMeta {
    id: String,
    digest: String,
    path: String,
    title: String,
    read: bool,
    tags: Vec<String>,
    published: String,
    created: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Manifest {
    notes: Vec<NoteMeta>,
}
impl Manifest {
    fn encode(&self) -> Vec<u8> {
        let notes: Vec<Value> = self
            .notes
            .iter()
            .map(|note| {
                ObjectBuilder::new()
                    .set("id", note.id.clone())
                    .set("digest", note.digest.clone())
                    .set("path", note.path.clone())
                    .set("title", note.title.clone())
                    .set("read", note.read)
                    .set("tags", note.tags.clone())
                    .set("published", note.published.clone())
                    .set("created", note.created.clone())
                    .build()
            })
            .collect();
        ObjectBuilder::new()
            .set("version", 1_u32)
            .set("notes", notes)
            .build()
            .to_json()
            .into_bytes()
    }
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "manifest is not UTF-8")?;
        let root = kobo_json::parse(text).map_err(|_| "manifest is not valid JSON")?;
        if root.get("version").and_then(Value::as_i64) != Some(1) {
            return Err("manifest version is unsupported".to_owned());
        }
        let rows = root
            .get("notes")
            .and_then(Value::as_array)
            .ok_or("manifest has no notes")?;
        let mut notes = Vec::new();
        for row in rows {
            let text_field = |key: &str| {
                row.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let id = text_field("id");
            let digest = text_field("digest");
            if !valid_id(&id) || !valid_digest(&digest) {
                return Err("manifest has an invalid note identity".to_owned());
            }
            let tags = row
                .get("tags")
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            notes.push(NoteMeta {
                id,
                digest,
                path: text_field("path"),
                title: text_field("title"),
                read: row.get("read").and_then(Value::as_bool).unwrap_or(false),
                tags,
                published: text_field("published"),
                created: text_field("created"),
            });
        }
        Ok(Self { notes })
    }
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix("note-").is_some_and(|suffix| {
        suffix.len() <= 27 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A path-derived, stable identity: re-pushing the same note (even with a
/// changed body) keeps the same shelf blob name, so an edit overwrites its
/// own file instead of leaving an orphaned copy under a stale digest.
fn note_id(path: &str) -> String {
    format!("note-{:016x}", fnv1a(path))
}

fn fnv1a(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

enum Target {
    Device(String),
    Sim,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("push") => push(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn push(arguments: &[String]) -> Result<(), String> {
    let (local, target) = parse_push(arguments)?;
    let notes = collect_notes(Path::new(local))?;
    let existing = match &target {
        Target::Device(host) => read_manifest(host)?,
        Target::Sim => read_local_manifest()?,
    };
    let mut manifest = Manifest::default();
    let mut changed = Vec::new();
    let mut total = 0_usize;
    for (path, raw) in &notes {
        let (frontmatter, body) = split_frontmatter(raw);
        let fields = parse_frontmatter(frontmatter);
        let scalar = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_scalar())
                .unwrap_or_default()
        };
        let tags = fields
            .iter()
            .find(|(k, _)| k == "tags")
            .map(|(_, v)| v.as_list())
            .unwrap_or_default();
        let body = body.trim_start_matches('\n');
        let digest = kobo_net::sha256::hex_digest(body.as_bytes());
        let id = note_id(path);
        let title = {
            let title = scalar("title");
            if title.is_empty() {
                derive_title(path)
            } else {
                title
            }
        };
        total = total
            .checked_add(body.len())
            .ok_or("this vault is too large to add up")?;
        if !existing
            .notes
            .iter()
            .any(|note| note.id == id && note.digest == digest)
        {
            changed.push((id.clone(), body.to_owned()));
        }
        manifest.notes.push(NoteMeta {
            id,
            digest,
            path: path.clone(),
            title,
            read: scalar("read") == "true",
            tags,
            published: scalar("published"),
            created: scalar("created"),
        });
    }
    if total > MAX_CAPACITY {
        return Err(format!(
            "this vault packs to {} MB of note bodies; Clippings keeps its shelf under {} MB",
            total / (1024 * 1024),
            MAX_CAPACITY / (1024 * 1024)
        ));
    }
    if manifest.encode().len() > MAX_MANIFEST {
        return Err(format!(
            "this vault has too many notes for one manifest (over {MAX_MANIFEST} bytes of metadata); split it into smaller folders"
        ));
    }
    let keep: std::collections::BTreeSet<&str> =
        manifest.notes.iter().map(|note| note.id.as_str()).collect();
    let removed: Vec<&str> = existing
        .notes
        .iter()
        .map(|note| note.id.as_str())
        .filter(|id| !keep.contains(id))
        .collect();
    match target {
        Target::Device(host) => {
            transfer(&host, &manifest, &changed, &removed)?;
        }
        Target::Sim => {
            publish_local(&manifest, &changed, &removed)?;
        }
    }
    println!(
        "Pushed {} note(s): {} updated, {} removed, {} unchanged",
        manifest.notes.len(),
        changed.len(),
        removed.len(),
        manifest.notes.len() - changed.len()
    );
    Ok(())
}

fn parse_push(arguments: &[String]) -> Result<(&str, Target), String> {
    let Some(local) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    match &arguments[1..] {
        [flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok((local, Target::Device(host.clone())))
        }
        [flag] if flag == "--sim" => Ok((local, Target::Sim)),
        _ => Err(USAGE.to_owned()),
    }
}

fn collect_notes(root: &Path) -> Result<Vec<(String, String)>, String> {
    let metadata = fs::metadata(root)
        .map_err(|error| format!("could not read {}: {error}", root.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let mut notes = Vec::new();
    visit(root, root, &mut notes)?;
    notes.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(notes)
}

fn visit(root: &Path, dir: &Path, notes: &mut Vec<(String, String)>) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|error| format!("read {}: {error}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|error| format!("read {}: {error}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            visit(root, &path, notes)?;
            continue;
        }
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped the clippings root", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let body = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        notes.push((relative, body));
    }
    Ok(())
}

fn derive_title(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md")
        .to_owned()
}

enum Field {
    Scalar(String),
    List(Vec<String>),
}
impl Field {
    fn as_scalar(&self) -> String {
        match self {
            Self::Scalar(value) => value.clone(),
            Self::List(_) => String::new(),
        }
    }
    fn as_list(&self) -> Vec<String> {
        match self {
            Self::List(items) => items.clone(),
            Self::Scalar(_) => Vec::new(),
        }
    }
}

/// Splits `---\n...\n---\n` frontmatter from the note body. A note without a
/// leading `---` block is treated as having no frontmatter and no metadata.
fn split_frontmatter(raw: &str) -> (&str, &str) {
    let Some(after_first) = raw.strip_prefix("---\n") else {
        return ("", raw);
    };
    if let Some(end) = after_first.find("\n---\n") {
        return (&after_first[..end], &after_first[end + 5..]);
    }
    if let Some(block) = after_first.strip_suffix("\n---") {
        return (block, "");
    }
    ("", raw)
}

/// A small, deliberately non-general YAML subset: `key: value`, `key: "value"`,
/// and `key:` followed by `  - item` lines. This is exactly the shape Obsidian
/// Web Clipper writes; a full YAML parser is not needed for it.
fn parse_frontmatter(block: &str) -> Vec<(String, Field)> {
    let mut fields = Vec::new();
    let mut lines = block.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let rest = rest.trim();
        if !rest.is_empty() {
            fields.push((key.to_owned(), Field::Scalar(unquote(rest))));
            continue;
        }
        let mut items = Vec::new();
        while let Some(next) = lines.peek() {
            let trimmed = next.trim_start();
            let Some(item) = trimmed
                .strip_prefix("- ")
                .or_else(|| (trimmed == "-").then_some(""))
            else {
                break;
            };
            items.push(unquote(item.trim()));
            lines.next();
        }
        if items.is_empty() {
            fields.push((key.to_owned(), Field::Scalar(String::new())));
        } else {
            fields.push((key.to_owned(), Field::List(items)));
        }
    }
    fields
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn sim_root() -> PathBuf {
    kobo_sim::simulated_data_root("clippings")
}

fn read_local_manifest() -> Result<Manifest, String> {
    let path = sim_root().join(MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("the simulator Clippings manifest is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(error) => Err(format!("read Clippings simulator shelf: {error}")),
    }
}

fn publish_local(
    manifest: &Manifest,
    changed: &[(String, String)],
    removed: &[&str],
) -> Result<(), String> {
    let root = sim_root();
    fs::create_dir_all(&root)
        .map_err(|error| format!("create Clippings simulator shelf: {error}"))?;
    for (id, body) in changed {
        let dest = root.join(format!("{id}.md"));
        let partial = root.join(format!(".{id}.md.writing"));
        fs::write(&partial, body).map_err(|error| format!("write Clippings note {id}: {error}"))?;
        fs::rename(&partial, &dest)
            .map_err(|error| format!("publish Clippings note {id}: {error}"))?;
    }
    let dest = root.join(MANIFEST);
    let partial = root.join(format!(".{MANIFEST}.writing"));
    fs::write(&partial, manifest.encode())
        .map_err(|error| format!("write Clippings manifest: {error}"))?;
    fs::rename(&partial, &dest).map_err(|error| format!("publish Clippings manifest: {error}"))?;
    for id in removed {
        let _ignored = fs::remove_file(root.join(format!("{id}.md")));
    }
    Ok(())
}

fn read_manifest(host: &str) -> Result<Manifest, String> {
    let output = remote(
        host,
        &format!("set -eu\nif [ -f '{ROOT}/{MANIFEST}' ]; then base64 '{ROOT}/{MANIFEST}'; fi\n"),
    )?;
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Manifest::default());
    }
    let bytes = base64_decode(&String::from_utf8_lossy(&output.stdout))?;
    Manifest::decode(&bytes)
        .map_err(|error| format!("the reader's Clippings manifest is invalid: {error}"))
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let cleaned = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if cleaned.len() % 4 != 0 {
        return Err("the reader returned malformed base64 Clippings data".to_owned());
    }
    let mut decoded = Vec::with_capacity(cleaned.len() / 4 * 3);
    for group in cleaned.chunks_exact(4) {
        let first = base64_value(group[0])?;
        let second = base64_value(group[1])?;
        let third = if group[2] == b'=' {
            None
        } else {
            Some(base64_value(group[2])?)
        };
        let fourth = if group[3] == b'=' {
            None
        } else {
            Some(base64_value(group[3])?)
        };
        if third.is_none() && fourth.is_some() {
            return Err("the reader returned malformed base64 Clippings data".to_owned());
        }
        decoded.push((first << 2) | (second >> 4));
        if let Some(third) = third {
            decoded.push((second << 4) | (third >> 2));
            if let Some(fourth) = fourth {
                decoded.push((third << 6) | fourth);
            }
        }
    }
    Ok(decoded)
}

fn base64_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err("the reader returned malformed base64 Clippings data".to_owned()),
    }
}

fn transfer(
    host: &str,
    manifest: &Manifest,
    changed: &[(String, String)],
    removed: &[&str],
) -> Result<(), String> {
    let mut script = format!("set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\n");
    for (id, body) in changed {
        let encoded = super::base64_encode(body.as_bytes());
        let _ = write!(
            script,
            "partial=\"$root/.{id}.md.writing\"\nbase64 -d > \"$partial\" <<'COBALT_CLIPPINGS_NOTE'\n{encoded}\nCOBALT_CLIPPINGS_NOTE\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{id}.md\"\n"
        );
    }
    let encoded = super::base64_encode(&manifest.encode());
    let _ = write!(
        script,
        "partial=\"$root/.{MANIFEST}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_CLIPPINGS_MANIFEST'\n{encoded}\nCOBALT_CLIPPINGS_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{MANIFEST}\"\nsync\n"
    );
    for id in removed {
        let _ = writeln!(script, "rm -f \"$root/{id}.md\"");
    }
    script.push_str("sync\n");
    let _ = remote(host, &script)?;
    Ok(())
}

fn remote(host: &str, script: &str) -> Result<super::RemoteShellOutput, String> {
    let output = super::run_remote_shell(&format!("root@{host}"), script, TRANSFER_TIMEOUT)
        .map_err(super::unreachable_device)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "the reader refused the Clippings transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "cobalt-clippings-cli-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("Clippings")).expect("fixture");
        fs::write(
            root.join("Clippings/Alpha.md"),
            "---\ntitle: \"Alpha\"\npublished: 2026-01-01\ntags:\n  - \"one\"\nread: false\n---\nAlpha body.\n",
        )
        .expect("alpha");
        fs::write(
            root.join("Clippings/Beta.md"),
            "---\ntitle: \"Beta\"\nread: true\n---\nBeta body.\n",
        )
        .expect("beta");
        root
    }

    #[test]
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
    }

    #[test]
    fn ids_are_stable_across_pushes_and_independent_of_content() {
        assert_eq!(note_id("A.md"), note_id("A.md"));
        assert_ne!(note_id("A.md"), note_id("B.md"));
        assert!(valid_id(&note_id("A.md")));
    }

    #[test]
    fn frontmatter_parses_into_the_manifest_fields() {
        let root = fixture();
        let notes = collect_notes(&root).expect("notes");
        assert_eq!(notes.len(), 2);
        let (path, raw) = notes.iter().find(|(p, _)| p.ends_with("Alpha.md")).unwrap();
        assert_eq!(path, "Clippings/Alpha.md");
        let (frontmatter, body) = split_frontmatter(raw);
        let fields = parse_frontmatter(frontmatter);
        let scalar = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_scalar())
                .unwrap_or_default()
        };
        assert_eq!(scalar("title"), "Alpha");
        assert_eq!(scalar("published"), "2026-01-01");
        assert_eq!(scalar("read"), "false");
        assert!(body.trim_start_matches('\n').starts_with("Alpha body."));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn manifest_round_trips_and_digests_are_valid() {
        let manifest = Manifest {
            notes: vec![NoteMeta {
                id: note_id("A.md"),
                digest: kobo_net::sha256::hex_digest(b"Alpha body.\n"),
                path: "A.md".to_owned(),
                title: "Alpha".to_owned(),
                read: false,
                tags: vec!["one".to_owned()],
                published: "2026-01-01".to_owned(),
                created: String::new(),
            }],
        };
        assert!(valid_digest(&manifest.notes[0].digest));
        let decoded = Manifest::decode(&manifest.encode()).expect("manifest decodes");
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn a_second_push_only_transfers_notes_whose_body_actually_changed() {
        let root = fixture();
        let notes = collect_notes(&root).expect("notes");
        let alpha_id = note_id("Clippings/Alpha.md");
        let alpha_digest = kobo_net::sha256::hex_digest(b"Alpha body.\n");
        let existing = Manifest {
            notes: vec![NoteMeta {
                id: alpha_id.clone(),
                digest: alpha_digest,
                path: "Clippings/Alpha.md".to_owned(),
                title: "Alpha".to_owned(),
                read: false,
                tags: vec!["one".to_owned()],
                published: "2026-01-01".to_owned(),
                created: String::new(),
            }],
        };
        let mut changed = Vec::new();
        for (path, raw) in &notes {
            let (_, body) = split_frontmatter(raw);
            let body = body.trim_start_matches('\n');
            let digest = kobo_net::sha256::hex_digest(body.as_bytes());
            let id = note_id(path);
            if !existing
                .notes
                .iter()
                .any(|note| note.id == id && note.digest == digest)
            {
                changed.push(id);
            }
        }
        assert_eq!(changed.len(), 1, "only Beta is new");
        assert_ne!(changed[0], alpha_id);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn refuses_a_file_as_a_clippings_root() {
        let path = std::env::temp_dir().join(format!(
            "cobalt-clippings-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::write(&path, "not a clippings folder").expect("file");
        assert!(collect_notes(&path).is_err());
        fs::remove_file(path).expect("cleanup");
    }
}
