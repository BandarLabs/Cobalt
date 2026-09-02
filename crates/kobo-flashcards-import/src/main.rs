#![forbid(unsafe_code)]

use kobo_flashcards_format::{DISTRIBUTION_DOCUMENTS, HOST_DEPENDENCY_LICENSES};
use kobo_flashcards_import::{
    export_local_review_log, import, stage_for_kobo, verify_bundle, ImportMode, ImportOptions,
};
use std::path::PathBuf;
use std::process::ExitCode;

const NOTICE: &str = "Flashcards is unofficial Anki-package compatibility software and is not affiliated with Ankitects or AnkiDroid. It contains no upstream logos. Anki is AGPL-3.0-or-later; AnkiDroid is GPL-3.0-or-later. See licenses/NOTICE-Flashcards-Anki.md.";

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flashcards-import: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[String]) -> Result<(), String> {
    if arguments == ["--notice"] {
        println!("{NOTICE}");
        return Ok(());
    }
    if arguments == ["--licenses"] {
        for (title, text) in DISTRIBUTION_DOCUMENTS {
            println!(
                "==============================================================================="
            );
            println!("{title}");
            println!("===============================================================================\n{text}");
        }
        println!("===============================================================================");
        println!("Flashcards host helper dependency licences");
        println!("===============================================================================\n{HOST_DEPENDENCY_LICENSES}");
        return Ok(());
    }
    if matches!(arguments, [operation, _] if operation == "verify") {
        return verify_command(&arguments[1]);
    }
    if matches!(arguments, [operation, _, flag, _] if operation == "stage" && flag == "--kobo-root")
    {
        return stage_command(&arguments[1], &arguments[3]);
    }
    if matches!(arguments, [operation, flag, root, output] if operation == "export-review-log" && flag == "--kobo-root")
    {
        return export_review_log_command(&arguments[2], &arguments[3]);
    }
    let [operation, input, flag, output, rest @ ..] = arguments else {
        return Err(usage().to_owned());
    };
    let mode = match (operation.as_str(), flag.as_str()) {
        ("import", "--merge") => ImportMode::MergeApkg,
        ("import", "--replace") => ImportMode::ReplaceColpkg,
        _ => return Err(usage().to_owned()),
    };
    let mut options = match mode {
        ImportMode::MergeApkg => ImportOptions::apkg(),
        ImportMode::ReplaceColpkg => ImportOptions::colpkg(),
    };
    match rest {
        [] => {}
        [merge_flag, source] if mode == ImportMode::MergeApkg && merge_flag == "--merge-into" => {
            options.merge_into = Some(PathBuf::from(source));
        }
        _ => return Err(usage().to_owned()),
    }
    let report = import(
        PathBuf::from(input).as_path(),
        PathBuf::from(output).as_path(),
        &options,
    )
    .map_err(|error| error.to_string())?;
    println!(
        "imported {}: {} notes, {} due cards ({} new, {} learning, {} review), {} decks, {} media files ({} bytes), {} image-bearing notes resolved, {} sound-bearing notes retained",
        report.package_kind,
        report.notes,
        report.active_cards,
        report.new_cards,
        report.learning_cards,
        report.review_cards,
        report.decks,
        report.media_files,
        report.media_bytes,
        report.image_bearing_notes,
        report.sound_bearing_notes,
    );
    println!("bundle sha256: {}", report.bundle_sha256);
    if !report.diagnostics.is_empty() {
        println!(
            "{} rendering diagnostic(s) retained in the bundle",
            report.diagnostics.len()
        );
    }
    Ok(())
}

fn verify_command(bundle: &str) -> Result<(), String> {
    let report =
        verify_bundle(PathBuf::from(bundle).as_path()).map_err(|error| error.to_string())?;
    println!(
        "verified: {} notes, {} due cards ({} new, {} learning, {} review), {} digest-verified media files; {} image-bearing notes resolve to bundled bytes; {} sound-bearing notes are retained",
        report.notes,
        report.active_cards,
        report.new_cards,
        report.learning_cards,
        report.review_cards,
        report.media_files,
        report.image_bearing_notes,
        report.sound_bearing_notes,
    );
    println!("bundle sha256: {}", report.bundle_sha256);
    Ok(())
}

fn stage_command(bundle: &str, root: &str) -> Result<(), String> {
    let report = stage_for_kobo(
        PathBuf::from(bundle).as_path(),
        PathBuf::from(root).as_path(),
        None,
    )
    .map_err(|error| error.to_string())?;
    println!(
        "staged {} digest-verified bytes to {}{}",
        report.bytes,
        report.destination.display(),
        if report.resumed_at == 0 {
            String::new()
        } else {
            format!(" (resumed at {} bytes)", report.resumed_at)
        }
    );
    println!("bundle sha256: {}", report.sha256);
    Ok(())
}

fn export_review_log_command(root: &str, output: &str) -> Result<(), String> {
    let report = export_local_review_log(
        PathBuf::from(root).as_path(),
        PathBuf::from(output).as_path(),
    )
    .map_err(|error| error.to_string())?;
    println!(
        "exported {} lossless Cobalt review records ({} bytes)",
        report.records, report.bytes
    );
    println!("review-log sha256: {}", report.sha256);
    Ok(())
}

const fn usage() -> &'static str {
    "usage: flashcards-import import INPUT.apkg --merge OUTPUT.cobfc [--merge-into EXISTING.cobfc]\n       flashcards-import import INPUT.colpkg --replace OUTPUT.cobfc\n       flashcards-import verify BUNDLE.cobfc\n       flashcards-import stage BUNDLE.cobfc --kobo-root MOUNT\n       flashcards-import export-review-log --kobo-root MOUNT OUTPUT.ndjson\n       flashcards-import --notice\n       flashcards-import --licenses"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_is_explicit_about_affiliation_and_licenses() {
        assert!(NOTICE.contains("unofficial"));
        assert!(NOTICE.contains("not affiliated"));
        assert!(NOTICE.contains("AGPL-3.0-or-later"));
        assert!(NOTICE.contains("GPL-3.0-or-later"));
        assert!(DISTRIBUTION_DOCUMENTS
            .iter()
            .all(|(_, document)| !document.is_empty()));
        assert!(!HOST_DEPENDENCY_LICENSES.is_empty());
    }
}
