//! Owner-attended Vault shelf management: walk a folder of Markdown notes on
//! the host and publish them over the already-paired SSH channel or into the
//! simulator. One direction: the host folder is the source of truth, and the
//! reader is a reader. Notes edited on the reader stay on the reader; nothing
//! here exports them back.

use kobo_vault_host::{
    digest, links_for, plan_adding, plan_prefixed, tags_for, title_for, ImportFailure,
    IncomingNote, Manifest, Push, MANIFEST, MAX_NOTES, SYNCED_MANIFEST, SYNCED_PREFIX,
};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/vault";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_INPUT_BYTES: u64 = 512 * 1024;
const SKIP_DIRS: &[&str] = &[".obsidian", ".trash", ".git", "node_modules"];
const USAGE: &str = concat!(
    "usage: kobo vault init ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo vault push VAULT_DIR [--exclude TEXT]... [--replace] ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo vault plan VAULT_DIR [--exclude TEXT]... [--replace] ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo vault ingest SYNCED_DIR [--exclude TEXT]... ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo vault preview NOTE.md\n\
                     \x20      kobo vault ls ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo vault rm ID ",
    crate::targets::target_choice!(),
    "\n\
                     \n\
                     push adds the folder's notes to the reader and updates the ones\n\
                     that changed; push again whenever your notes change. Only changed\n\
                     notes are sent, and a renamed note keeps its place. --replace\n\
                     makes the reader hold exactly this folder and removes every other\n\
                     pushed note.\n\
                     \n\
                     ingest packs a folder that another tool delivered onto a separate\n\
                     shelf; push and ingest never overwrite each other.\n",
    crate::targets::saved_reader_note!(),
);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Device(String),
    Sim,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("init") => init(&arguments[1..]),
        Some("push") => push(&arguments[1..], false),
        Some("plan") => push(&arguments[1..], true),
        Some("ingest") => ingest(&arguments[1..]),
        Some("preview") => preview(&arguments[1..]),
        Some("ls") => list(&arguments[1..]),
        Some("rm") => remove(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_target(arguments: &[String]) -> Result<Target, String> {
    match arguments {
        [flag] if flag == "--sim" => Ok(Target::Sim),
        [flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok(Target::Device(host.clone()))
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_push(arguments: &[String]) -> Result<(String, Vec<String>, Target), String> {
    let Some(folder) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    let mut excludes = Vec::new();
    let mut target = None;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--exclude" => {
                let Some(text) = arguments.get(index + 1) else {
                    return Err(USAGE.to_owned());
                };
                if text.is_empty() {
                    return Err("--exclude needs a non-empty text".to_owned());
                }
                excludes.push(text.clone());
                index += 2;
            }
            "--sim" => {
                if target.is_some() {
                    return Err(USAGE.to_owned());
                }
                target = Some(Target::Sim);
                index += 1;
            }
            flag if super::is_device_flag(flag) => {
                if target.is_some() {
                    return Err(USAGE.to_owned());
                }
                let Some(host) = arguments.get(index + 1) else {
                    return Err(USAGE.to_owned());
                };
                if !super::valid_device_host(host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                target = Some(Target::Device(host.clone()));
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    match target {
        Some(target) => Ok((folder.clone(), excludes, target)),
        None => Err(USAGE.to_owned()),
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    match parse_target(arguments)? {
        Target::Device(host) => {
            let output = remote(
                &host,
                &format!(
                    "set -eu\nmkdir -p '{ROOT}'\nchmod 700 '{ROOT}'\nsync\nprintf 'Vault shelf ready; transfers use this owner-attended SSH connection.\\n'\n"
                ),
            )?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        Target::Sim => {
            let root = sim_root();
            fs::create_dir_all(&root)
                .map_err(|error| format!("create Vault simulator shelf: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                    .map_err(|error| format!("protect Vault simulator shelf: {error}"))?;
            }
            println!(
                "Vault shelf ready at {}; transfers stay on this computer.",
                root.display()
            );
        }
    }
    Ok(())
}

pub(crate) struct Walk {
    pub(crate) offered: Vec<IncomingNote>,
    pub(crate) excluded: Vec<(String, String)>,
    pub(crate) failures: Vec<ImportFailure>,
}

/// Walk the vault folder into offers: Markdown files become notes keyed by
/// their vault-relative path, dot-directories and the usual tooling folders
/// are skipped, and each --exclude text removes its matches loudly so a plan
/// shows exactly what was left out and why.
pub(crate) fn walk(folder: &Path, excludes: &[String]) -> Result<Walk, String> {
    let metadata = fs::metadata(folder)
        .map_err(|error| format!("could not read {}: {error}", folder.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", folder.display()));
    }
    let mut paths = Vec::new();
    visit(folder, folder, &mut paths)?;
    paths.sort();
    let mut walk = Walk {
        offered: Vec::new(),
        excluded: Vec::new(),
        failures: Vec::new(),
    };
    for relative in paths {
        if let Some(rule) = excludes
            .iter()
            .find(|rule| relative.to_lowercase().contains(&rule.to_lowercase()))
        {
            walk.excluded.push((relative, rule.clone()));
            continue;
        }
        let full = folder.join(&relative);
        let size = fs::metadata(&full)
            .map_err(|error| format!("read {}: {error}", full.display()))?
            .len();
        if size > MAX_INPUT_BYTES {
            walk.failures.push(ImportFailure {
                input: relative,
                reason: format!("larger than the {MAX_INPUT_BYTES} byte note limit"),
            });
            continue;
        }
        match fs::read(&full) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(body) => {
                    let added = fs::metadata(&full)
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |duration| duration.as_secs());
                    walk.offered.push(IncomingNote {
                        title: title_for(&relative, &body),
                        tags: tags_for(&body),
                        links: links_for(&body),
                        digest: digest(body.as_bytes()),
                        added,
                        body: body.into_bytes(),
                        path: relative,
                    });
                }
                Err(_) => walk.failures.push(ImportFailure {
                    input: relative,
                    reason: "not UTF-8 text".to_owned(),
                }),
            },
            Err(error) => walk.failures.push(ImportFailure {
                input: relative,
                reason: format!("could not be read: {error}"),
            }),
        }
    }
    Ok(walk)
}

fn visit(root: &Path, dir: &Path, paths: &mut Vec<String>) -> Result<(), String> {
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
        if name.starts_with('.') || SKIP_DIRS.contains(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            visit(root, &path, paths)?;
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
            .map_err(|_| format!("{} escaped the vault root", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains('\n') {
            return Err(format!(
                "{} is not a usable note path for the Vault shelf",
                path.display()
            ));
        }
        paths.push(relative);
    }
    Ok(())
}

fn sim_root() -> PathBuf {
    kobo_sim::simulated_data_root("vault")
}

/// Key, record layout and cap of the packed index the pre-shelf Vault app
/// reads. Pushes keep that index current so a reader still on the old app
/// sees the vault until the multi-shelf app ships; the shelf stays the
/// source of truth.
const LEGACY_INDEX_KEY: &str = "vault-index-v1";
const LEGACY_INDEX_SEPARATOR: &str = "\n\n---vault-note---\n\n";
const LEGACY_MAX_INDEX: usize = 256 * 1024;
const LEGACY_DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/state/vault";

fn legacy_sim_root() -> PathBuf {
    kobo_sim::simulated_state_root("vault")
}

fn legacy_pack(notes: &[(String, String)]) -> Option<String> {
    let packed = notes
        .iter()
        .map(|(path, body)| format!("{path}\n{body}"))
        .collect::<Vec<_>>()
        .join(LEGACY_INDEX_SEPARATOR);
    (packed.len() <= LEGACY_MAX_INDEX).then_some(packed)
}

/// Rebuild the legacy packed index from every shelf manifest, in the format
/// the pre-shelf app decodes. Over-cap vaults skip the index: the old app
/// cannot hold them either way, and the new app never reads it.
fn legacy_write_sim() -> Result<(), String> {
    let mut notes: Vec<(String, String)> = Vec::new();
    for manifest_name in [MANIFEST, SYNCED_MANIFEST] {
        let manifest = read_local_manifest_named(manifest_name)?;
        for note in &manifest.notes {
            let file = sim_root().join(Push::note_name(&note.id));
            let body = fs::read_to_string(&file)
                .map_err(|error| format!("read Vault note {}: {error}", note.path))?;
            notes.push((note.path.clone(), body));
        }
    }
    let Some(packed) = legacy_pack(&notes) else {
        println!("Vault exceeds the old app's packed-index cap; only the new app shows it.");
        return Ok(());
    };
    let root = legacy_sim_root();
    fs::create_dir_all(&root).map_err(|error| format!("create Vault simulator store: {error}"))?;
    let dest = root.join(LEGACY_INDEX_KEY);
    let partial = root.join(format!(".{LEGACY_INDEX_KEY}.writing"));
    fs::write(&partial, packed)
        .and_then(|()| fs::rename(&partial, &dest))
        .map_err(|error| format!("publish Vault legacy index: {error}"))?;
    Ok(())
}

/// Same index for a device push: every pushed note's body is still local, so
/// the packed index ships in the same SSH session.
fn legacy_write_device(
    host: &str,
    folder: &str,
    notes: &[kobo_vault_host::NoteEntry],
) -> Result<(), String> {
    let mut packed_notes: Vec<(String, String)> = Vec::new();
    for note in notes {
        let body = fs::read_to_string(Path::new(folder).join(&note.path))
            .map_err(|error| format!("read {}: {error}", note.path))?;
        packed_notes.push((note.path.clone(), body));
    }
    let Some(packed) = legacy_pack(&packed_notes) else {
        println!("Vault exceeds the old app's packed-index cap; only the new app shows it.");
        return Ok(());
    };
    let encoded = super::base64_encode(packed.as_bytes());
    let script = format!(
        "set -eu\n\
         root='{LEGACY_DEVICE_ROOT}'\n\
         mkdir -p \"$root\"\n\
         chmod 700 \"$root\"\n\
         partial=\"$root/.{LEGACY_INDEX_KEY}.writing\"\n\
         base64 -d > \"$partial\" <<'KOBO_VAULT_INDEX'\n\
         {encoded}\n\
         KOBO_VAULT_INDEX\n\
         chmod 600 \"$partial\"\n\
         mv -f \"$partial\" \"$root/{LEGACY_INDEX_KEY}\"\n\
         sync\n"
    );
    remote(host, &script)?;
    Ok(())
}

fn read_local_manifest() -> Result<Manifest, String> {
    read_local_manifest_named(MANIFEST)
}

fn read_local_manifest_named(manifest_name: &str) -> Result<Manifest, String> {
    let path = sim_root().join(manifest_name);
    match fs::read(&path) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("the simulator Vault manifest is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(error) => Err(format!("read Vault simulator shelf: {error}")),
    }
}

fn publish_local(push: &Push) -> Result<(), String> {
    publish_local_named(push, MANIFEST)
}

fn publish_local_named(push: &Push, manifest_name: &str) -> Result<(), String> {
    let root = sim_root();
    fs::create_dir_all(&root).map_err(|error| format!("create Vault simulator shelf: {error}"))?;
    for prepared in &push.notes {
        let name = Push::note_name(&prepared.note_id);
        let dest = root.join(&name);
        let partial = root.join(format!(".{name}.writing"));
        fs::write(&partial, &prepared.markdown)
            .map_err(|error| format!("write Vault note {name}: {error}"))?;
        fs::rename(&partial, &dest)
            .map_err(|error| format!("publish Vault note {name}: {error}"))?;
    }
    let dest = root.join(manifest_name);
    let partial = root.join(format!(".{manifest_name}.writing"));
    fs::write(&partial, push.manifest.encode())
        .map_err(|error| format!("write Vault manifest: {error}"))?;
    fs::rename(&partial, &dest).map_err(|error| format!("publish Vault manifest: {error}"))?;
    for removed in &push.removed {
        let _ = fs::remove_file(root.join(Push::note_name(&removed.id)));
    }
    Ok(())
}

pub(crate) fn print_plan(push: &Push, walk: &Walk) {
    let fresh = push.notes.len();
    for note in &push.manifest.notes {
        let state = if push.renamed.iter().any(|rename| rename.id == note.id) {
            "renamed"
        } else if push
            .notes
            .iter()
            .any(|prepared| prepared.note_id == note.id)
        {
            "new"
        } else {
            "unchanged"
        };
        println!(
            "{state} {} · {} · {} byte(s)",
            note.id, note.path, note.bytes
        );
    }
    for rename in &push.renamed {
        println!("rename {} -> {} (no transfer)", rename.from, rename.to);
    }
    for removed in &push.removed {
        println!("remove {} · {}", removed.id, removed.path);
    }
    for (path, rule) in &walk.excluded {
        println!("excluded {path} (matches \"{rule}\")");
    }
    for failure in &push.manifest.failures {
        println!("could not import {}: {}", failure.input, failure.reason);
    }
    println!(
        "{} note(s) on the shelf, {fresh} to transfer.",
        push.manifest.notes.len()
    );
}

fn push(arguments: &[String], plan_only: bool) -> Result<(), String> {
    let (arguments, replace) = without_replace(arguments);
    let (folder, excludes, target) = parse_push(&arguments)?;
    let mut walk = walk(Path::new(&folder), &excludes)?;
    let existing = match &target {
        Target::Device(host) => read_manifest(host)?,
        Target::Sim => read_local_manifest()?,
    };
    let failures = std::mem::take(&mut walk.failures);
    let offered = std::mem::take(&mut walk.offered);
    let push = if replace {
        plan_prefixed(&existing, offered, failures, "note-")?
    } else {
        plan_adding(&existing, offered, failures, "note-")?
    };
    print_plan(&push, &walk);
    if push.manifest.notes.len() > MAX_NOTES {
        return Err(format!("Vault holds at most {MAX_NOTES} notes"));
    }
    if plan_only {
        println!("Plan only. Nothing was transferred or removed.");
        return Ok(());
    }
    match &target {
        Target::Device(host) => {
            transfer(host, &push)?;
            legacy_write_device(host, &folder, &push.manifest.notes)?;
            let actual = read_manifest(host)?;
            if actual.notes != push.manifest.notes {
                return Err("the reader's Vault shelf does not match the published plan".to_owned());
            }
            println!(
                "Transferred {} note file(s); the reader's shelf now matches the plan.",
                push.notes.len()
            );
            print!("{}", on_the_reader(&push.manifest));
        }
        Target::Sim => {
            publish_local(&push)?;
            legacy_write_sim()?;
            let actual = read_local_manifest()?;
            if actual.notes != push.manifest.notes {
                return Err(
                    "the simulator Vault shelf does not match the published plan".to_owned(),
                );
            }
            println!(
                "Transferred {} note file(s); the simulator shelf now matches the plan.",
                push.notes.len()
            );
            print!("{}", on_the_reader(&push.manifest));
        }
    }
    Ok(())
}

/// Takes `--replace` out of a push's arguments. Only push and plan know it,
/// and the shared parser stays the one ingest uses.
fn without_replace(arguments: &[String]) -> (Vec<String>, bool) {
    let replace = arguments.iter().any(|argument| argument == "--replace");
    let rest = arguments
        .iter()
        .filter(|argument| *argument != "--replace")
        .cloned()
        .collect();
    (rest, replace)
}

/// What the reader holds after a push, by top-level folder, and when the
/// app will show it, so the push ends with the one next step.
fn on_the_reader(manifest: &Manifest) -> String {
    let mut folders: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for note in &manifest.notes {
        let folder = note
            .path
            .split_once('/')
            .map_or("(top level)", |(folder, _)| folder);
        *folders.entry(folder).or_default() += 1;
    }
    let count = manifest.notes.len();
    let mut text = format!(
        "Now on the reader: {count} note{}.\n",
        if count == 1 { "" } else { "s" }
    );
    for (folder, notes) in folders {
        let _ = writeln!(text, "  {folder}: {notes}");
    }
    text.push_str("Vault shows them the next time you come back to it. Push again whenever your notes change.\n");
    text
}

fn list(arguments: &[String]) -> Result<(), String> {
    let (pushed, synced) = match parse_target(arguments)? {
        Target::Device(host) => (
            read_manifest_named(&host, MANIFEST)?,
            read_manifest_named(&host, SYNCED_MANIFEST)?,
        ),
        Target::Sim => (
            read_local_manifest_named(MANIFEST)?,
            read_local_manifest_named(SYNCED_MANIFEST)?,
        ),
    };
    if pushed.notes.is_empty() && synced.notes.is_empty() {
        println!("The Vault shelf is empty.");
    }
    for note in &pushed.notes {
        println!("{} · {} · {} byte(s)", note.id, note.path, note.bytes);
    }
    for note in &synced.notes {
        println!(
            "{} · {} · {} byte(s) · synced",
            note.id, note.path, note.bytes
        );
    }
    for failure in pushed.failures.iter().chain(synced.failures.iter()) {
        println!("could not import {}: {}", failure.input, failure.reason);
    }
    Ok(())
}

/// Pack a folder Sync has delivered onto the synced shelf. The same walk,
/// plan and atomic publish as push, against the separate synced manifest so
/// a sync drop and a hand push never overwrite each other.
fn ingest(arguments: &[String]) -> Result<(), String> {
    let (folder, excludes, target) = parse_push(arguments)?;
    let mut walk = walk(Path::new(&folder), &excludes)?;
    let existing = match &target {
        Target::Device(host) => read_manifest_named(host, SYNCED_MANIFEST)?,
        Target::Sim => read_local_manifest_named(SYNCED_MANIFEST)?,
    };
    let failures = std::mem::take(&mut walk.failures);
    let offered = std::mem::take(&mut walk.offered);
    let push = plan_prefixed(&existing, offered, failures, SYNCED_PREFIX)?;
    print_plan(&push, &walk);
    if push.manifest.notes.len() > MAX_NOTES {
        return Err(format!("Vault holds at most {MAX_NOTES} synced notes"));
    }
    match &target {
        Target::Device(host) => {
            transfer_named(host, &push, SYNCED_MANIFEST)?;
            let actual = read_manifest_named(host, SYNCED_MANIFEST)?;
            if actual.notes != push.manifest.notes {
                return Err(
                    "the reader's synced Vault shelf does not match the ingestion plan".to_owned(),
                );
            }
            println!(
                "Ingested {} synced note file(s); the reader's synced shelf matches the plan.",
                push.notes.len()
            );
            println!("Vault indexes the synced shelf when it next opens.");
        }
        Target::Sim => {
            publish_local_named(&push, SYNCED_MANIFEST)?;
            legacy_write_sim()?;
            let actual = read_local_manifest_named(SYNCED_MANIFEST)?;
            if actual.notes != push.manifest.notes {
                return Err(
                    "the simulator synced Vault shelf does not match the ingestion plan".to_owned(),
                );
            }
            println!(
                "Ingested {} synced note file(s); the simulator synced shelf matches the plan.",
                push.notes.len()
            );
            println!("Vault indexes the synced shelf when it next opens.");
        }
    }
    Ok(())
}

fn remove(arguments: &[String]) -> Result<(), String> {
    let (id, target) = match arguments {
        [id, flag] if flag == "--sim" => (id.clone(), Target::Sim),
        [id, flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            (id.clone(), Target::Device(host.clone()))
        }
        _ => return Err(USAGE.to_owned()),
    };
    let mut manifest = match &target {
        Target::Device(host) => read_manifest(host)?,
        Target::Sim => read_local_manifest()?,
    };
    let Some(position) = manifest.notes.iter().position(|note| note.id == id) else {
        return Err(format!("no Vault note named {id}"));
    };
    let removed = manifest.notes.remove(position);
    // Every entry sharing the note file leaves with it.
    manifest.notes.retain(|note| note.id != removed.id);
    let push = Push {
        manifest,
        notes: Vec::new(),
        removed: vec![removed.clone()],
        renamed: Vec::new(),
    };
    match &target {
        Target::Device(host) => {
            transfer(host, &push)?;
            println!("The old app's packed index refreshes on the next push.");
        }
        Target::Sim => {
            publish_local(&push)?;
            legacy_write_sim()?;
        }
    }
    println!("Removed {} from the Vault shelf.", removed.path);
    Ok(())
}

/// Show roughly how a note paginates at the reader's dimensions. The rendered
/// text is measured against the panel at body size; the app sets headings
/// larger, so a note with many headings can run a page or two longer there.
fn preview(arguments: &[String]) -> Result<(), String> {
    let [path] = arguments else {
        return Err(USAGE.to_owned());
    };
    let body =
        fs::read_to_string(path).map_err(|error| format!("could not read {path}: {error}"))?;
    let rendered = render_markdown(&body);
    let context = kobo_sdk::Context::default();
    let pages = context.paginate_reading(&rendered, true);
    let last = pages
        .last()
        .and_then(|page| page.last())
        .map_or("(empty note)", String::as_str);
    let title = title_for(path, &body);
    println!("{title}");
    println!(
        "{} page(s) at reader dimensions; the last page ends with:",
        pages.len()
    );
    println!("  {last}");
    Ok(())
}

/// The same Markdown boundary the app uses, kept local so the preview
/// measures rendered text rather than raw source.
fn render_markdown(markdown: &str) -> String {
    let options = pulldown_cmark::Options::ENABLE_TABLES
        | pulldown_cmark::Options::ENABLE_FOOTNOTES
        | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
        | pulldown_cmark::Options::ENABLE_TASKLISTS;
    let mut html = String::new();
    pulldown_cmark::html::push_html(
        &mut html,
        pulldown_cmark::Parser::new_ext(markdown, options),
    );
    // The shelf format's own note ceiling: a note the shelf accepted is a
    // document, not a feed field, so the preview measures all of it.
    kobo_html::to_text_within(
        &html,
        usize::try_from(kobo_vault_host::MAX_NOTE_BYTES).unwrap_or(usize::MAX),
    )
}

fn read_manifest(host: &str) -> Result<Manifest, String> {
    read_manifest_named(host, MANIFEST)
}

fn read_manifest_named(host: &str, manifest_name: &str) -> Result<Manifest, String> {
    let output = remote(
        host,
        &format!("cat '{ROOT}/{manifest_name}' 2>/dev/null || true\n"),
    )?;
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(Manifest::default());
    }
    Manifest::decode(text.trim().as_bytes())
        .map_err(|error| format!("the reader's Vault manifest is invalid: {error}"))
}

fn transfer(host: &str, push: &Push) -> Result<(), String> {
    transfer_named(host, push, MANIFEST)
}

fn transfer_named(host: &str, push: &Push, manifest_name: &str) -> Result<(), String> {
    let mut script = format!("set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\n");
    for prepared in &push.notes {
        let name = Push::note_name(&prepared.note_id);
        let encoded = super::base64_encode(&prepared.markdown);
        let _ = write!(
            script,
            "partial=\"$root/.{name}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_VAULT_NOTE'\n{encoded}\nCOBALT_VAULT_NOTE\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{name}\"\n"
        );
    }
    let encoded = super::base64_encode(&push.manifest.encode());
    let _ = write!(
        script,
        "partial=\"$root/.{manifest_name}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_VAULT_MANIFEST'\n{encoded}\nCOBALT_VAULT_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{manifest_name}\"\nsync\n"
    );
    for removed in &push.removed {
        let _ = writeln!(script, "rm -f \"$root/{}\"", Push::note_name(&removed.id));
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
        Err(super::unreachable_if_ssh_gave_up(
            super::remote_shell_error(
                format!("Vault transfer on {host} exited with {}", output.status),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn flag_like_hosts_fail_before_input_or_network() {
        let base = ["/nonexistent", "--exclude", "fixture"]
            .map(str::to_owned)
            .to_vec();
        for flag in ["--device", "-s"] {
            for host in ["--sim", "--device", "-s", "-reader", ""] {
                let target = [flag.to_owned(), host.to_owned()];
                assert!(parse_target(&target).unwrap_err().contains("device host"));
                let remove_args = ["fixture".to_owned(), flag.to_owned(), host.to_owned()];
                assert!(remove(&remove_args).unwrap_err().contains("device host"));
                for first in [false, true] {
                    let mut arguments = base.clone();
                    let index = if first { 1 } else { arguments.len() };
                    arguments.splice(index..index, [flag.to_owned(), host.to_owned()]);
                    let error = parse_push(&arguments).expect_err("invalid host rejected");
                    assert!(error.contains("device host"), "{arguments:?}: {error}");
                }
            }
        }
    }

    #[test]
    fn a_push_adds_unless_it_is_asked_to_replace() {
        let arguments = ["notes", "--replace", "--sim"].map(str::to_owned).to_vec();
        let (rest, replace) = without_replace(&arguments);
        assert!(replace);
        assert_eq!(rest, ["notes", "--sim"]);
        assert!(!without_replace(&rest).1);
    }

    #[test]
    fn a_push_ends_by_counting_the_notes_on_the_reader_by_folder() {
        let note = |path: &str| kobo_vault_host::NoteEntry {
            id: format!("note-{path}"),
            path: path.to_owned(),
            title: path.to_owned(),
            tags: Vec::new(),
            links: Vec::new(),
            digest: "d".to_owned(),
            bytes: 1,
            added: 1,
        };
        let manifest = Manifest {
            notes: vec![
                note("Welcome.md"),
                note("Projects/A.md"),
                note("Projects/B.md"),
            ],
            failures: Vec::new(),
        };
        let text = on_the_reader(&manifest);
        assert!(text.starts_with("Now on the reader: 3 notes.\n  (top level): 1\n  Projects: 2\n"));
        assert!(!text.contains("kobo sync"));
    }

    #[test]
    fn single_destinations_remain_valid() {
        for flags in [&["--sim"][..], &["--device", "fixture"]] {
            let mut arguments = vec!["/nonexistent".to_owned()];
            arguments.extend(flags.iter().map(|value| (*value).to_owned()));
            assert!(parse_push(&arguments).is_ok());
        }
    }
    #[test]
    fn ambiguous_destinations_fail_before_input_is_read() {
        for flags in [
            &["--sim", "--sim"][..],
            &["--sim", "--device", "fixture"],
            &["--device", "fixture", "--sim"],
            &["--device", "fixture", "--device", "fixture"],
        ] {
            let mut arguments: Vec<String> = ["/nonexistent"]
                .iter()
                .map(|value| (*value).to_owned())
                .collect();
            arguments.extend(flags.iter().map(|value| (*value).to_owned()));
            assert_eq!(parse_push(&arguments).err().unwrap(), USAGE);
        }
    }

    use super::*;
}
