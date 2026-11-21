//! Every name the runtime keeps under its state folder is one no application
//! may take.
//!
//! An application's saved state lives in a folder named after its
//! identifier, and the runtime keeps records of its own in the same folder.
//! A package published under one of those names would be handed the record
//! as its own state. The names are reserved in `kobo-app-store`, and each
//! module that has such a record checks its own name against that list.
//!
//! That was not enough once already. The time zone record was added, with
//! its path helper and no such check, after the list was written, and
//! nothing failed. A check that each module has to remember to write does
//! not catch the module that did not know to. So this one reads the
//! sources: it finds every name this crate reaches for directly under the
//! state folder and holds each to the list. It knows the two ways a record
//! is named here, a join onto the state folder and a constant that begins
//! with it. A record named a third way is not found, which is why the
//! records are all named one of these two.

use std::fs;
use std::path::Path;

/// What follows the state folder in a path built from a root.
const JOINED: &str = concat!("join(\"state\")", ".join(");
/// How a constant that names a path under the state folder begins.
const WRITTEN: &str = concat!("\"state", "/");

/// The names `source` keeps directly under the state folder.
///
/// Only the code is read, not its tests, which make up folders of their
/// own. A name given by a variable is an application's own folder and is
/// passed over: those are the identifiers the list is there to keep away
/// from these.
fn names_kept(source: &str) -> Vec<String> {
    let code = source.split("#[cfg(test)]").next().unwrap_or_default();
    let mut names = Vec::new();
    for (at, _) in code.match_indices(JOINED) {
        let argument = &code[at + JOINED.len()..];
        let argument = argument.split(')').next().unwrap_or_default().trim();
        if let Some(literal) = argument
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            names.push(literal.to_owned());
        } else if !argument.is_empty()
            && argument
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            let value = constant(code, argument).unwrap_or_else(|| {
                panic!("{argument} is joined onto the state folder and is not a constant here")
            });
            names.push(value);
        }
    }
    for line in code
        .lines()
        .filter(|line| line.trim_start().starts_with("const "))
    {
        if let Some((_, after)) = line.split_once(WRITTEN) {
            let name = after.split(['/', '"']).next().unwrap_or_default();
            names.push(name.to_owned());
        }
    }
    names
}

/// The text of the string constant `name` in `code`.
fn constant(code: &str, name: &str) -> Option<String> {
    let declared = format!("const {name}: &str = \"");
    let (_, after) = code.split_once(&declared)?;
    Some(after.split('"').next()?.to_owned())
}

/// Every Rust source under `folder`, with its text.
fn sources(folder: &Path, found: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(folder).expect("the sources").flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path
                .file_name()
                .is_some_and(|name| name != "state_names.rs")
        {
            let text = fs::read_to_string(&path).expect("a source");
            found.push((path.display().to_string(), text));
        }
    }
}

#[test]
fn every_record_the_runtime_keeps_under_its_state_folder_has_a_reserved_name() {
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut kept = Vec::new();
    for (file, text) in &files {
        for name in names_kept(text) {
            assert!(
                kobo_app_store::is_public_reserved_app_id(&name),
                "{file} keeps {name:?} under the state folder, where an application \
                 published as {name:?} would be given it as its own state. Add the name to \
                 PUBLIC_RESERVED_APP_IDS in kobo-app-store."
            );
            kept.push(name);
        }
    }
    kept.sort();
    kept.dedup();
    // The records there are today. If this list comes up short, the sources
    // have changed shape and this test has stopped seeing them, which is a
    // failure of the test and must not pass for a clean result.
    assert_eq!(
        kept,
        [
            "accepted-untested",
            "app-link",
            "auto-update",
            "last-update-error",
            "notices",
            "time-zone",
            "touch-calibration",
        ]
    );
}

#[test]
fn a_record_is_found_however_it_is_named_and_an_applications_folder_is_not() {
    let source = "const RECORD: &str = \"moon-phase\";\n\
                  const INBOX: &str = \"state/letters\";\n\
                  fn a(root: &Path) -> PathBuf { root.join(\"state\").join(RECORD) }\n\
                  fn b(root: &Path) -> PathBuf { root.join(\"state\").join(\"tide-table\") }\n\
                  fn c(root: &Path, app: &str) -> PathBuf { root.join(\"state\").join(app) }\n\
                  fn d(root: &Path) -> Store { Store::new(root.join(\"state\")) }\n\
                  #[cfg(test)]\n\
                  mod tests { fn e(root: &Path) -> PathBuf { root.join(\"state\").join(\"made-up\") } }\n";
    assert_eq!(names_kept(source), ["moon-phase", "tide-table", "letters"]);
}
