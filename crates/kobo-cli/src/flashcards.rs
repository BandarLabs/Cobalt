//! Flashcards commands delegate to the separately distributed host importer.
//! The CLI does not link Anki or reinterpret collection contents.

use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE: &str = "usage: kobo flashcards import DECK.apkg --merge COLLECTION.cobfc [--merge-into EXISTING.cobfc]\n\
                     \x20      kobo flashcards import COLLECTION.colpkg --replace COLLECTION.cobfc\n\
                     \x20      kobo flashcards verify COLLECTION.cobfc\n\
                     \x20      kobo flashcards stage COLLECTION.cobfc --kobo-root MOUNT\n\
                     \x20      kobo flashcards export-review-log --kobo-root MOUNT OUTPUT.ndjson\n\
                     \x20      kobo flashcards status\n\
                     \x20      kobo flashcards formats\n\
                     \x20      kobo flashcards --licenses\n\n\
                     Uses the separate flashcards-import helper. Install it beside kobo or on PATH.\n\
                     Import prepares a local bundle; stage transfers it to a mounted reader.\n\
                     Keep Flashcards closed while staging. Review logs are preserved separately.";

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
            "The Flashcards importer is not installed. Install the separate flashcards-import helper beside kobo or on PATH, then retry. Source-build instructions: apps/flashcards/README.md. No import or transfer was started.".to_owned()
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
    let path = helper();
    let forwarded = helper_arguments(arguments)?;
    if arguments == ["status"] {
        println!("Helper: {}", path.display());
        run_helper(&path, &forwarded)?;
        return run_helper(&path, &["--notice".to_owned()]);
    }
    run_helper(&path, &forwarded)
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
    fn status_uses_the_helpers_own_version() {
        assert_eq!(
            helper_arguments(&args(&["status"])).unwrap(),
            args(&["--version"])
        );
    }
}
