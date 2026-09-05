//! Owner-attended packing of an Obsidian vault into the key Vault reads.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: kobo vault init (--device IP | --sim)\n\
                     \x20      kobo vault push LOCAL_DIR (--device IP | --sim | --out INDEX)";
const INDEX_KEY: &str = "vault-index-v1";
const INDEX_SEPARATOR: &str = "\n\n---vault-note---\n\n";
const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/state/vault";
const MAX_INDEX: usize = 256 * 1024;
const SKIP_DIRS: &[&str] = &[".obsidian", ".trash", ".git", "node_modules"];

enum Destination<'a> {
    Device(&'a str),
    Simulator,
    File(&'a str),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("init") => init(&arguments[1..]),
        Some("push") => push(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    match parse_destination(arguments, true)? {
        Destination::Device(host) => {
            let output = remote(
                host,
                &format!(
                    "set -eu\nmkdir -p '{DEVICE_ROOT}'\nchmod 700 '{DEVICE_ROOT}'\nsync\nprintf 'Vault shelf ready.\\n'\n"
                ),
            )?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
            Ok(())
        }
        Destination::Simulator => {
            fs::create_dir_all(sim_root())
                .map_err(|error| format!("create simulator vault store: {error}"))?;
            println!("Vault simulator store ready: {}", sim_root().display());
            Ok(())
        }
        Destination::File(_) => Err(USAGE.to_owned()),
    }
}

fn push(arguments: &[String]) -> Result<(), String> {
    let (local, destination) = parse_push(arguments)?;
    let notes = collect_notes(Path::new(local))?;
    let index = encode_index(&notes)?;
    match destination {
        Destination::Device(host) => transfer(&index, host, notes.len()),
        Destination::Simulator => {
            write_index(&sim_root().join(INDEX_KEY), &index)?;
            println!(
                "Pushed {} note(s) to the simulator store ({INDEX_KEY}, {} bytes).",
                notes.len(),
                index.len()
            );
            Ok(())
        }
        Destination::File(path) => {
            write_index(Path::new(path), &index)?;
            println!(
                "Packed {} note(s) into {path} ({} bytes).",
                notes.len(),
                index.len()
            );
            Ok(())
        }
    }
}

fn parse_push(arguments: &[String]) -> Result<(&str, Destination<'_>), String> {
    let Some(local) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    let destination = parse_destination(&arguments[1..], false)?;
    Ok((local, destination))
}

fn parse_destination(arguments: &[String], init: bool) -> Result<Destination<'_>, String> {
    let mut host = None;
    let mut sim = false;
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            flag if super::is_device_flag(flag) => {
                host = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            "--sim" => {
                sim = true;
                index += 1;
            }
            "--out" if !init => {
                output = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    match (host, sim, output) {
        (Some(host), false, None) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok(Destination::Device(host))
        }
        (None, true, None) => Ok(Destination::Simulator),
        (None, false, Some(path)) => Ok(Destination::File(path)),
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
        if name.starts_with('.') || SKIP_DIRS.contains(&name) {
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
            .map_err(|_| format!("{} escaped the vault root", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains('\n') || relative.contains(INDEX_SEPARATOR) {
            return Err(format!(
                "{} is not a usable note path for the Vault index",
                path.display()
            ));
        }
        let body = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if body.contains(INDEX_SEPARATOR) {
            return Err(format!(
                "{} contains the Vault index separator; rename that heading or split the note",
                path.display()
            ));
        }
        notes.push((relative, body));
    }
    Ok(())
}

fn encode_index(notes: &[(String, String)]) -> Result<String, String> {
    let encoded = notes
        .iter()
        .map(|(path, body)| format!("{path}\n{body}"))
        .collect::<Vec<_>>()
        .join(INDEX_SEPARATOR);
    if encoded.len() > MAX_INDEX {
        return Err(format!(
            "this vault packs to {} bytes; Vault v1 keeps the index in the app store, which stops at {MAX_INDEX} bytes",
            encoded.len()
        ));
    }
    Ok(encoded)
}

fn write_index(path: &Path, index: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} is not a usable index path", path.display()))?;
    let temporary = path.with_file_name(format!(".{name}.writing"));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.write_all(index.as_bytes())
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("flush {}: {error}", temporary.display()))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ignored = fs::remove_file(&temporary);
        format!("publish {}: {error}", path.display())
    })
}

fn transfer(index: &str, host: &str, notes: usize) -> Result<(), String> {
    let encoded = super::base64_encode(index.as_bytes());
    let script = format!(
        "set -eu\n\
         root='{DEVICE_ROOT}'\n\
         mkdir -p \"$root\"\n\
         chmod 700 \"$root\"\n\
         partial=\"$root/.{INDEX_KEY}.writing\"\n\
         base64 -d > \"$partial\" <<'KOBO_VAULT_INDEX'\n\
         {encoded}\n\
         KOBO_VAULT_INDEX\n\
         chmod 600 \"$partial\"\n\
         mv -f \"$partial\" \"$root/{INDEX_KEY}\"\n\
         sync\n\
         printf 'Pushed {notes} Vault note(s)\\n'\n"
    );
    let output = remote(host, &script)?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn remote(host: &str, script: &str) -> Result<super::RemoteShellOutput, String> {
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the Vault transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output)
}

fn sim_root() -> PathBuf {
    env::temp_dir().join("cobalt-sim-state").join("vault")
}

#[cfg(test)]
mod tests {
    use super::{collect_notes, encode_index, INDEX_SEPARATOR};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root =
            std::env::temp_dir().join(format!("cobalt-vault-cli-{}-{unique}", std::process::id()));
        fs::create_dir_all(root.join("Projects")).expect("fixture");
        fs::create_dir_all(root.join(".obsidian")).expect("obsidian");
        fs::create_dir_all(root.join("attachments")).expect("attachments");
        fs::write(root.join(".obsidian/app.json"), "{}").expect("workspace");
        fs::write(
            root.join("attachments/sketch.png"),
            [0x89, 0x50, 0x4e, 0x47],
        )
        .expect("png");
        fs::write(
            root.join("Welcome.md"),
            "# Welcome\n\nHome note. See [[Alpha]].\n",
        )
        .expect("welcome");
        fs::write(
            root.join("Projects/Alpha.md"),
            "# Alpha\n\nA project with a wiki link to [[Welcome]].\n\n---\n\n#project\n",
        )
        .expect("alpha");
        root
    }

    #[test]
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
    }

    #[test]
    fn packs_markdown_and_skips_obsidian_and_attachments() {
        let root = fixture();
        let notes = collect_notes(&root).expect("notes");
        assert_eq!(
            notes
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["Projects/Alpha.md", "Welcome.md"]
        );
        let index = encode_index(&notes).expect("index");
        assert!(index.contains("Welcome.md"));
        assert!(index.contains("[[Alpha]]"));
        assert!(index.contains("---\n\n#project"));
        assert!(!index.contains("app.json"));
        assert!(!index.contains("sketch.png"));
        assert!(index.contains(INDEX_SEPARATOR));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn empty_directory_packs_to_an_empty_index() {
        let root = std::env::temp_dir().join(format!(
            "cobalt-vault-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir_all(&root).expect("empty");
        let notes = collect_notes(&root).expect("notes");
        assert!(notes.is_empty());
        assert_eq!(encode_index(&notes).expect("index"), "");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn refuses_a_file_as_a_vault() {
        let path = std::env::temp_dir().join(format!(
            "cobalt-vault-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::write(&path, "not a vault").expect("file");
        assert!(collect_notes(&path).is_err());
        fs::remove_file(path).expect("cleanup");
    }
}
