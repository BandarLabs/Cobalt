//! A diagnostic report an owner can attach to a question.
//!
//! The report answers "what is this setup?" for whoever helps, while
//! treating the owner's own data as theirs: no file contents, no book or
//! photo names, no trust material, no keys, no full serials. Counts and
//! kinds travel; content does not. `--include-paths` adds the file names a
//! send named, for the cases where the path is the question - the owner
//! asks for that explicitly, it is never the default.

use std::fmt::Write as _;
use std::path::PathBuf;

/// Builds the report text from the stores this computer keeps.
#[must_use]
pub fn build(include_paths: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Cobalt diagnostic report");
    let _ = writeln!(out, "kobo {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        out,
        "os: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        out,
        "redacted: file contents, book and photo names, trust material, keys, full serials{}",
        if include_paths { "" } else { ", file paths" }
    );
    out.push('\n');

    let config = config_dir();
    match crate::readers::Store::load(&config.join("readers")) {
        Ok(store) => {
            let _ = writeln!(out, "saved readers: {}", store.readers.len());
            for reader in &store.readers {
                let _ = writeln!(
                    out,
                    "  {} (serial {}…, paired: {})",
                    reader.nickname,
                    reader.serial.get(..4).unwrap_or(&reader.serial),
                    if reader.paired { "yes" } else { "no" }
                );
            }
        }
        Err(error) => {
            let _ = writeln!(out, "saved readers: unreadable ({error})");
        }
    }
    match crate::steps::Steps::load(&config.join("setup-steps")) {
        Ok(steps) => {
            let names: Vec<&str> = steps.steps.iter().map(|step| step.name.as_str()).collect();
            let _ = writeln!(out, "completed setup steps: {}", names.join(", "));
        }
        Err(error) => {
            let _ = writeln!(out, "completed setup steps: unreadable ({error})");
        }
    }
    match crate::receipts::Receipts::load(&config.join("receipts")) {
        Ok(ledger) => {
            let _ = writeln!(out, "sends on record: {}", ledger.receipts.len());
            for receipt in &ledger.receipts {
                if include_paths {
                    let _ = writeln!(
                        out,
                        "  {} -> {} ({})",
                        receipt.file, receipt.target, receipt.app
                    );
                } else {
                    let _ = writeln!(out, "  (a file) -> {} ({})", receipt.target, receipt.app);
                }
            }
        }
        Err(error) => {
            let _ = writeln!(out, "sends on record: unreadable ({error})");
        }
    }
    match crate::receipts::Pending::load(&config.join("pending-send")) {
        Ok(Some(pending)) => {
            if include_paths {
                let _ = writeln!(
                    out,
                    "a send is waiting to be retried: to {} ({})",
                    pending.target,
                    pending.app.as_deref().unwrap_or("undecided companion")
                );
            } else {
                let _ = writeln!(
                    out,
                    "a send is waiting to be retried ({})",
                    pending.app.as_deref().unwrap_or("undecided companion")
                );
            }
        }
        Ok(None) => {
            let _ = writeln!(out, "no send is waiting to be retried");
        }
        Err(error) => {
            let _ = writeln!(out, "pending send: unreadable ({error})");
        }
    }
    out
}

/// The folder the stores live in, so the report reads what the CLI wrote.
fn config_dir() -> PathBuf {
    crate::readers::store_path()
        .parent()
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_default_report_carries_no_paths_or_full_serials() {
        // Point the stores at a fixture config with identifiable content.
        let dir = std::env::temp_dir().join(format!("kobo-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("readers"),
            "reader = clara\nserial = N365410043013\naddress = 10.0.0.8\npaired = yes\n\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("receipts"),
            "send = /home/owner/private/beach.png\napp = frame\ntarget = sim\nsha256 = abc\nat = 1\n\n",
        )
        .unwrap();
        // Safety: the environment variable is process-global, so this test
        // sets it, builds, and restores before anything else can interleave
        // in this test process.
        std::env::set_var("KOBO_CONFIG_DIR", &dir);
        let report = super::build(false);
        std::env::remove_var("KOBO_CONFIG_DIR");
        assert!(report.contains("clara"));
        assert!(report.contains("N365"));
        assert!(!report.contains("N365410043013"), "{report}");
        assert!(!report.contains("beach.png"), "{report}");
        assert!(
            !report.contains("10.0.0.8"),
            "addresses stay local: {report}"
        );
        std::env::set_var("KOBO_CONFIG_DIR", &dir);
        let with_paths = super::build(true);
        std::env::remove_var("KOBO_CONFIG_DIR");
        assert!(with_paths.contains("beach.png"), "{with_paths}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
