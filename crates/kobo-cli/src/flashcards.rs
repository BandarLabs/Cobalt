//! Flashcards commands delegate to the separately distributed host importer.
//! The CLI does not link Anki or reinterpret collection contents.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const USAGE: &str = concat!(
    "usage: kobo flashcards preview COLLECTION.cobfc --out PREVIEW.html [--card NUMBER]\n\
                     \x20      kobo flashcards import DECK.apkg --merge COLLECTION.cobfc [--merge-into EXISTING.cobfc]\n\
                     \x20      kobo flashcards import COLLECTION.colpkg --replace COLLECTION.cobfc\n\
                     \x20      kobo flashcards verify COLLECTION.cobfc\n\
                     \x20      kobo flashcards push COLLECTION.cobfc ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo flashcards stage COLLECTION.cobfc --kobo-root MOUNT\n\
                     \x20      kobo flashcards export-review-log --kobo-root MOUNT OUTPUT.ndjson\n\
                     \x20      kobo flashcards status\n\
                     \x20      kobo flashcards formats\n\
                     \x20      kobo flashcards --licenses\n\n\
                     Uses the signed flashcards-import helper installed beside kobo. Source builds may also use PATH.\n\
                     Import prepares a local bundle. Push sends it over Wi-Fi; stage copies it to a reader plugged in over USB.\n\
                     The reader holds one collection: add decks to it with --merge-into before pushing.\n\
                     Grades on the reader are kept when a collection is sent again.\n",
    crate::targets::saved_reader_note!(),
);

/// The one collection the reader studies, beside the grades it keeps.
const BUNDLE: &str = "collection.cobfc";
const REVIEW_LOG: &str = "cobalt-review-log.ndjson";
const DEVICE_DATA: &str = "/mnt/onboard/.adds/cobalt/data/flashcards";
const MAX_BUNDLE: u64 = 32 * 1024 * 1024;
/// The first bytes of every collection `import` writes.
const MAGIC: &[u8] = b"CBFLASH\0";

fn helper() -> PathBuf {
    if let Some(path) = std::env::var_os("KOBO_FLASHCARDS_IMPORT") {
        return PathBuf::from(path);
    }
    if let Ok(executable) = std::env::current_exe() {
        let sibling = executable.with_file_name("flashcards-import");
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from("flashcards-import")
}

fn helper_arguments(arguments: &[String]) -> Result<Vec<String>, String> {
    let values = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    match values.as_slice() {
        ["status"] => Ok(vec!["--version".to_owned()]),
        ["formats"] => Ok(vec!["--formats".to_owned()]),
        ["--licenses" | "--notice"]
        | ["preview", _, "--out", _]
        | ["preview", _, "--out", _, "--card", _]
        | ["verify", _]
        | ["stage", _, "--kobo-root", _]
        | ["export-review-log", "--kobo-root", _, _]
        | ["import", _, "--merge" | "--replace", _]
        | ["import", _, "--merge", _, "--merge-into", _] => Ok(arguments.to_vec()),
        // Preserve the earlier APKG spelling, without silently replacing a collection.
        ["import", input, "--out", output]
            if Path::new(input)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("apkg")) =>
        {
            Ok(vec![
                "import".into(),
                (*input).into(),
                "--merge".into(),
                (*output).into(),
            ])
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn run_helper(path: &Path, arguments: &[String]) -> Result<(), String> {
    let status = Command::new(path).args(arguments).status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "The Flashcards importer is not installed or is missing from this host installation. Run 'kobo update' to restore the signed helper, or follow the source-build instructions in apps/flashcards/README.md. No import or transfer was started.".to_owned()
        } else {
            format!("Could not start the Flashcards importer: {error}")
        }
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("The Flashcards importer did not complete ({status}). Review its message above before retrying."))
    }
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    if arguments.first().map(String::as_str) == Some("push") {
        return push(&arguments[1..]);
    }
    let path = helper();
    let forwarded = helper_arguments(arguments)?;
    if arguments == ["status"] {
        println!("Helper: {}", path.display());
        run_helper(&path, &forwarded)?;
        return run_helper(&path, &["--notice".to_owned()]);
    }
    run_helper(&path, &forwarded)
}

#[derive(Debug, Eq, PartialEq)]
enum Target {
    Sim,
    Device(String),
}

fn parse_push(arguments: &[String]) -> Result<(&str, Target), String> {
    let mut file = None;
    let mut target = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        if argument == "--sim" {
            if target.replace(Target::Sim).is_some() {
                return Err(USAGE.to_owned());
            }
            index += 1;
        } else if super::is_device_flag(argument) {
            let host = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            if target.replace(Target::Device(host.clone())).is_some() {
                return Err(USAGE.to_owned());
            }
            index += 2;
        } else if argument.starts_with('-') || file.replace(argument).is_some() {
            return Err(USAGE.to_owned());
        } else {
            index += 1;
        }
    }
    match (file, target) {
        (Some(file), Some(target)) => Ok((file, target)),
        _ => Err(USAGE.to_owned()),
    }
}

/// Reads a collection and refuses anything that is not one before it goes
/// anywhere: an Anki package sent as it is would only fail on the reader,
/// where the owner can do nothing about it.
fn read_collection(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if metadata.len() > MAX_BUNDLE {
        return Err(format!(
            "{} is larger than the 32 MB a reader can open. Import fewer decks into it.",
            path.display()
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if !bytes.starts_with(MAGIC) {
        let anki = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("apkg") || extension.eq_ignore_ascii_case("colpkg")
            });
        return Err(if anki {
            format!(
                "{} is an Anki package. Import it first, then push the collection:\n  kobo flashcards import {} --merge collection.cobfc\n  kobo flashcards push collection.cobfc",
                path.display(),
                path.display()
            )
        } else {
            format!(
                "{} is not a Flashcards collection. Make one with kobo flashcards import.",
                path.display()
            )
        });
    }
    Ok(bytes)
}

/// Sends a collection over Wi-Fi, or into the simulator, in place of the
/// one the reader has. The reader studies one collection, so adding a deck
/// happens on the computer with `import --merge-into`; the grades the reader
/// has recorded live in their own file and are never touched.
fn push(arguments: &[String]) -> Result<(), String> {
    let (file, target) = parse_push(arguments)?;
    let bytes = read_collection(Path::new(file))?;
    let (size, grades) = match target {
        Target::Sim => push_local(&kobo_sim::simulated_data_root("flashcards"), &bytes)?,
        Target::Device(host) => push_remote(&host, &bytes)?,
    };
    println!("Sent {file} to Flashcards.");
    println!(
        "On the reader: one collection of {}, and {} kept in its review log.",
        megabytes(size),
        match grades {
            1 => "1 grade".to_owned(),
            count => format!("{count} grades"),
        }
    );
    println!("Flashcards opens it the next time it starts or the reader wakes.");
    Ok(())
}

fn push_local(root: &Path, bytes: &[u8]) -> Result<(u64, usize), String> {
    fs::create_dir_all(root).map_err(|error| format!("create {}: {error}", root.display()))?;
    let partial = root.join(format!(".{BUNDLE}.{}.writing", std::process::id()));
    fs::write(&partial, bytes).map_err(|error| format!("write {}: {error}", partial.display()))?;
    fs::rename(&partial, root.join(BUNDLE)).map_err(|error| {
        let _ = fs::remove_file(&partial);
        format!("publish {BUNDLE}: {error}")
    })?;
    let grades = fs::read(root.join(REVIEW_LOG))
        .map(|log| log.split(|byte| *byte == b'\n').skip(1).count())
        .unwrap_or(0);
    Ok((bytes.len() as u64, grades))
}

fn push_remote(host: &str, bytes: &[u8]) -> Result<(u64, usize), String> {
    let script = push_script(bytes);
    // A large collection takes a while to cross Wi-Fi as base64.
    let timeout = super::REMOTE_COMMAND_TIMEOUT
        + Duration::from_secs(u64::try_from(bytes.len() / 200_000).unwrap_or(u64::MAX));
    let output = super::run_remote_shell(&format!("root@{host}"), &script, timeout)
        .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the collection: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let report = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| {
        report
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    Ok((
        field("collection "),
        usize::try_from(field("grades ")).unwrap_or(usize::MAX),
    ))
}

fn push_script(bytes: &[u8]) -> String {
    let encoded = super::base64_encode(bytes);
    let count = bytes.len();
    let digest = kobo_net::sha256::hex_digest(bytes);
    format!(
        "set -eu\nroot='{DEVICE_DATA}'\nmkdir -p \"$root\"\npartial=\"$root/.{BUNDLE}.$$.writing\"\ntrap 'rm -f \"$partial\"' EXIT HUP INT TERM\nbase64 -d > \"$partial\" <<'KOBO_FLASHCARDS'\n{encoded}\nKOBO_FLASHCARDS\ntest \"$(wc -c < \"$partial\")\" = '{count}'\nset -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{BUNDLE}\"\nsync\nprintf 'collection %s\\n' \"$(wc -c < \"$root/{BUNDLE}\")\"\nif test -f \"$root/{REVIEW_LOG}\"; then printf 'grades %s\\n' \"$(wc -l < \"$root/{REVIEW_LOG}\")\"; else printf 'grades 0\\n'; fi\n"
    )
}

fn megabytes(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a collection is at most 32 MB, well inside f64"
        )]
        let megabytes = bytes as f64 / (1024.0 * 1024.0);
        format!("{megabytes:.1} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn merge_preserves_paths_and_existing_collection_arguments() {
        let values = args(&[
            "import",
            "deck with spaces.apkg",
            "--merge",
            "new.cobfc",
            "--merge-into",
            "existing.cobfc",
        ]);
        assert_eq!(helper_arguments(&values).unwrap(), values);
    }

    #[test]
    fn legacy_output_spelling_never_implies_collection_replacement() {
        assert_eq!(
            helper_arguments(&args(&["import", "deck.apkg", "--out", "deck.cobfc"])).unwrap(),
            args(&["import", "deck.apkg", "--merge", "deck.cobfc"])
        );
        assert!(helper_arguments(&args(&[
            "import",
            "collection.colpkg",
            "--out",
            "deck.cobfc"
        ]))
        .is_err());
    }

    #[test]
    fn missing_helper_has_actionable_guidance() {
        let error = run_helper(
            Path::new("/nonexistent-cobalt-test/flashcards-import"),
            &args(&["--notice"]),
        )
        .unwrap_err();
        assert!(error.contains("not installed"));
        assert!(error.contains("No import or transfer was started"));
    }

    #[test]
    fn push_takes_one_collection_and_one_reader_in_any_order() {
        assert_eq!(
            parse_push(&args(&["deck.cobfc", "--sim"])).unwrap(),
            ("deck.cobfc", Target::Sim)
        );
        assert_eq!(
            parse_push(&args(&["--device", "10.0.0.2", "deck.cobfc"])).unwrap(),
            ("deck.cobfc", Target::Device("10.0.0.2".to_owned()))
        );
        assert!(parse_push(&args(&["deck.cobfc"])).is_err());
        assert!(parse_push(&args(&["a.cobfc", "b.cobfc", "--sim"])).is_err());
        assert!(parse_push(&args(&["deck.cobfc", "--sim", "--device", "10.0.0.2"])).is_err());
    }

    #[test]
    fn an_anki_package_is_turned_away_with_the_import_command() {
        let root = std::env::temp_dir().join(format!("flashcards-push-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let package = root.join("Spanish.apkg");
        fs::write(&package, b"PK\x03\x04").unwrap();
        let error = read_collection(&package).unwrap_err();
        assert!(error.contains("kobo flashcards import"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_push_replaces_the_collection_and_keeps_the_grades() {
        let root = std::env::temp_dir().join(format!("flashcards-sim-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(REVIEW_LOG), b"{}\n{}\n").unwrap();
        fs::write(root.join(BUNDLE), b"CBFLASH\0old").unwrap();
        let (size, grades) = push_local(&root, b"CBFLASH\0newer").unwrap();
        assert_eq!((size, grades), (13, 2));
        assert_eq!(fs::read(root.join(BUNDLE)).unwrap(), b"CBFLASH\0newer");
        assert_eq!(fs::read(root.join(REVIEW_LOG)).unwrap(), b"{}\n{}\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_device_script_checks_the_bytes_and_leaves_the_review_log_alone() {
        let script = push_script(b"CBFLASH\0bundle");
        assert!(script.contains("sha256sum"));
        assert!(script.contains("mv -f \"$partial\" \"$root/collection.cobfc\""));
        assert!(!script.contains("rm -f \"$root/cobalt-review-log"));
        assert!(script.contains("wc -l < \"$root/cobalt-review-log.ndjson\""));
    }

    #[test]
    fn status_uses_the_helpers_own_version() {
        assert_eq!(
            helper_arguments(&args(&["status"])).unwrap(),
            args(&["--version"])
        );
    }
}
