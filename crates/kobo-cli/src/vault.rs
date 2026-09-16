//! Owner-attended Vault shelf management: walk a folder of Markdown notes on
//! the host and publish them over the already-paired SSH channel or into the
//! simulator. One direction: the host folder is the source of truth, and the
//! reader is a reader. Notes edited on the reader stay on the reader; nothing
//! here exports them back.

use kobo_vault_host::{
    digest, plan, title_for, ImportFailure, IncomingNote, Manifest, Push, MANIFEST, MAX_NOTES,
};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/vault";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_INPUT_BYTES: u64 = 512 * 1024;
const SKIP_DIRS: &[&str] = &[".obsidian", ".trash", ".git", "node_modules"];
const USAGE: &str = "usage: kobo vault init (--sim | --device IP)\n\
                     \x20      kobo vault push VAULT_DIR [--exclude TEXT]... (--sim | --device IP)\n\
                     \x20      kobo vault plan VAULT_DIR [--exclude TEXT]... (--sim | --device IP)\n\
                     \x20      kobo vault preview NOTE.md\n\
                     \x20      kobo vault ls (--sim | --device IP)\n\
                     \x20      kobo vault rm ID (--sim | --device IP)\n\
                     \n\
                     The host folder is the source of truth: push mirrors it onto the\n\
                     shelf. Notes the folder no longer offers leave the shelf, and a\n\
                     renamed note keeps its place without a second transfer. Vault\n\
                     reads Markdown; edits made on the reader are not exported.";

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
                target = Some(Target::Sim);
                index += 1;
            }
            flag if super::is_device_flag(flag) => {
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

struct Walk {
    offered: Vec<IncomingNote>,
    excluded: Vec<(String, String)>,
    failures: Vec<ImportFailure>,
}

/// Walk the vault folder into offers: Markdown files become notes keyed by
/// their vault-relative path, dot-directories and the usual tooling folders
/// are skipped, and each --exclude text removes its matches loudly so a plan
/// shows exactly what was left out and why.
fn walk(folder: &Path, excludes: &[String]) -> Result<Walk, String> {
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

fn read_local_manifest() -> Result<Manifest, String> {
    let path = sim_root().join(MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("the simulator Vault manifest is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(error) => Err(format!("read Vault simulator shelf: {error}")),
    }
}

fn publish_local(push: &Push) -> Result<(), String> {
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
    let dest = root.join(MANIFEST);
    let partial = root.join(format!(".{MANIFEST}.writing"));
    fs::write(&partial, push.manifest.encode())
        .map_err(|error| format!("write Vault manifest: {error}"))?;
    fs::rename(&partial, &dest).map_err(|error| format!("publish Vault manifest: {error}"))?;
    for removed in &push.removed {
        let _ = fs::remove_file(root.join(Push::note_name(&removed.id)));
    }
    Ok(())
}

fn print_plan(push: &Push, walk: &Walk) {
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
    let (folder, excludes, target) = parse_push(arguments)?;
    let mut walk = walk(Path::new(&folder), &excludes)?;
    let existing = match &target {
        Target::Device(host) => read_manifest(host)?,
        Target::Sim => read_local_manifest()?,
    };
    let failures = std::mem::take(&mut walk.failures);
    let offered = std::mem::take(&mut walk.offered);
    let push = plan(&existing, offered, failures)?;
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
            let actual = read_manifest(host)?;
            if actual.notes != push.manifest.notes {
                return Err("the reader's Vault shelf does not match the published plan".to_owned());
            }
            println!(
                "Transferred {} note file(s); the reader's shelf now matches the plan.",
                push.notes.len()
            );
            println!("Vault indexes the shelf when it next opens.");
            println!(
                "To keep this folder current without re-pushing: kobo sync setup {folder} --folder vault --device {host}"
            );
        }
        Target::Sim => {
            publish_local(&push)?;
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
            println!("Vault indexes the shelf when it next opens.");
        }
    }
    Ok(())
}

fn list(arguments: &[String]) -> Result<(), String> {
    let manifest = match parse_target(arguments)? {
        Target::Device(host) => read_manifest(&host)?,
        Target::Sim => read_local_manifest()?,
    };
    if manifest.notes.is_empty() {
        println!("The Vault shelf is empty.");
    }
    for note in &manifest.notes {
        println!("{} · {} · {} byte(s)", note.id, note.path, note.bytes);
    }
    for failure in &manifest.failures {
        println!("could not import {}: {}", failure.input, failure.reason);
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
        Target::Device(host) => transfer(host, &push)?,
        Target::Sim => publish_local(&push)?,
    }
    println!("Removed {} from the Vault shelf.", removed.path);
    Ok(())
}

/// Show how a note paginates at the reader's dimensions: the rendered text is
/// measured against the panel the same way the app measures it, so the page
/// count and the closing line are what the reader will see.
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
    kobo_html::to_text_within(&html, usize::try_from(kobo_vault_host::MAX_NOTE_BYTES).unwrap_or(usize::MAX))
}

fn read_manifest(host: &str) -> Result<Manifest, String> {
    let output = remote(
        host,
        &format!("cat '{ROOT}/{MANIFEST}' 2>/dev/null || true\n"),
    )?;
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(Manifest::default());
    }
    Manifest::decode(text.trim().as_bytes())
        .map_err(|error| format!("the reader's Vault manifest is invalid: {error}"))
}

fn transfer(host: &str, push: &Push) -> Result<(), String> {
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
        "partial=\"$root/.{MANIFEST}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_VAULT_MANIFEST'\n{encoded}\nCOBALT_VAULT_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{MANIFEST}\"\nsync\n"
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
