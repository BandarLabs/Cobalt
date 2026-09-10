//! Receive an owner-prepared export through the existing paired SSH connection.
use kobo_sdk::exports::{Offer, MAX_OFFER_BYTES, OFFER_KEY};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

const USAGE: &str = "Receive the copy you prepared with Export in a reader app.\n\nusage: kobo export --app APP (--device ADDRESS | --sim) --out FOLDER\n\nThe reader keeps its copy. Existing computer files are never replaced.\nFor a reader, use the SSH connection enabled during kobo setup.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    let (mut app, mut device, mut output) = (None, None, None);
    let mut simulated = false;
    let mut args = arguments.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--app" if app.is_none() => app = Some(args.next().ok_or(USAGE)?.as_str()),
            "--device" if device.is_none() => device = Some(args.next().ok_or(USAGE)?.as_str()),
            "--out" if output.is_none() => output = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--sim" if !simulated => simulated = true,
            _ => return Err(USAGE.into()),
        }
    }
    let app = app
        .filter(|app| kobo_protocol::valid_app_id(app))
        .ok_or(USAGE)?;
    let output = output.ok_or(USAGE)?;
    if simulated == device.is_some() || device.is_some_and(|host| !super::valid_device_host(host)) {
        return Err(USAGE.into());
    }
    let read_offer = |key: &str, maximum: usize, data: bool| -> Result<Vec<u8>, String> {
        if let Some(host) = device {
            remote_read(host, app, key, maximum, data)
        } else {
            let root = if data {
                kobo_sim::simulated_data_root(app)
            } else {
                std::env::temp_dir().join("cobalt-sim-state").join(app)
            };
            bounded_file(&root.join(key), maximum)
        }
    };
    let offer = Offer::restore(&read_offer(OFFER_KEY, MAX_OFFER_BYTES, false)?)?;
    let bytes = read_offer(&offer.digest, offer.bytes, true)?;
    if !offer.matches(&bytes) {
        return Err("The received copy is incomplete or changed. Choose Export in the app and try again. Your existing files were kept.".into());
    }
    let destination = publish(&output, app, &offer, &bytes)?;
    println!(
        "Saved {}\nThe original remains on your reader.",
        destination.display()
    );
    Ok(())
}

fn bounded_file(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let before = fs::symlink_metadata(path)
        .map_err(|_| "No export is ready. Choose Export in the app first.")?;
    if !before.file_type().is_file() || before.len() > maximum as u64 {
        return Err("The export file is not a supported size or file type.".into());
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if before.dev() != opened.dev() || before.ino() != opened.ino() {
        return Err("The export file changed. Try again.".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > maximum {
        return Err("The export file changed. Try again.".into());
    }
    Ok(bytes)
}

fn remote_script(app: &str, key: &str, maximum: usize, data: bool) -> String {
    // Both path components have been validated; no owner text enters shell code.
    let folder = if data { "data" } else { "state" };
    let root = format!("/mnt/onboard/.adds/cobalt/{folder}");
    let app_root = if data && app == "audiobook" {
        "/mnt/onboard/Audiobooks".to_owned()
    } else {
        format!("{root}/{app}")
    };
    let parents = if data && app == "audiobook" {
        "/mnt/onboard".to_owned()
    } else {
        format!("/mnt/onboard/.adds /mnt/onboard/.adds/cobalt {root}")
    };
    format!("set -e\nfor p in {parents} {app_root}; do test -d \"$p\" && test ! -L \"$p\" || exit 1; done\ntest -f '{app_root}/{key}' && test ! -L '{app_root}/{key}' || exit 1\nhead -c {} '{app_root}/{key}'\n", maximum + 1)
}

fn remote_read(
    host: &str,
    app: &str,
    key: &str,
    maximum: usize,
    data: bool,
) -> Result<Vec<u8>, String> {
    let script = remote_script(app, key, maximum, data);
    let mut command = super::remote_shell_command(&format!("root@{host}"));
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not connect to the reader: {error}"))?;
    let stdin = child.stdin.take();
    let mut stdin = super::take_remote_pipe(&mut child, stdin, "stdin")?;
    let stdout = child.stdout.take();
    let stdout = super::take_remote_pipe(&mut child, stdout, "stdout")?;
    let stderr = child.stderr.take();
    let stderr = super::take_remote_pipe(&mut child, stderr, "stderr")?;
    let writer = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(maximum as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(4096).read_to_end(&mut bytes).map(|_| bytes)
    });
    let status =
        super::wait_for_remote_child(&mut child, "receive export", Duration::from_secs(60));
    let sent = writer
        .join()
        .map_err(|_| "The connection stopped while requesting the copy.")?;
    let bytes = reader
        .join()
        .map_err(|_| "The connection stopped while receiving the copy.")?
        .map_err(|error| error.to_string())?;
    let _diagnostics = errors.join().map_err(|_| "The connection stopped.")?;
    if status.is_err() || status.is_ok_and(|status| !status.success()) || sent.is_err() {
        return Err("Could not receive this copy. Wake the reader, check its paired connection, and choose Export in the app again.".into());
    }
    if bytes.len() > maximum {
        return Err(
            "The reader returned more data than expected. Existing files were kept.".into(),
        );
    }
    Ok(bytes)
}

fn filename(title: &str, app: &str) -> String {
    let mut name = String::new();
    for character in title.chars().take(80) {
        let character = if character.is_alphanumeric() || matches!(character, '-' | '_' | ' ') {
            character
        } else {
            '_'
        };
        if name.len() + character.len_utf8() > 160 {
            break;
        }
        name.push(character);
    }
    let name = name.trim_matches([' ', '_', '-']);
    if name.is_empty() {
        app.into()
    } else {
        name.into()
    }
}

fn publish(folder: &Path, app: &str, offer: &Offer, bytes: &[u8]) -> Result<PathBuf, String> {
    if !offer.matches(bytes) {
        return Err("The export did not pass its file check.".into());
    }
    fs::create_dir_all(folder)
        .map_err(|error| format!("Could not open the receiving folder: {error}"))?;
    let folder = folder.canonicalize().map_err(|error| error.to_string())?;
    let base = filename(&offer.title, app);
    let temporary = folder.join(format!(
        ".cobalt-export-{}-{}.part",
        std::process::id(),
        offer.digest
    ));
    let mut file = fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&temporary)
        .map_err(|_| "A previous transfer may still be running. Choose another receiving folder or remove its unfinished .part file after checking it.")?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("The computer could not save this copy: {error}"))?;
        // Publish with a no-replace link. A concurrent export can never overwrite
        // a file that appeared after the existence check.
        for index in 0..100 {
            let suffix = if index == 0 {
                String::new()
            } else {
                format!(" ({})", index + 1)
            };
            let target = folder.join(format!("{base}{suffix}.{}", offer.format.extension()));
            match fs::hard_link(&temporary, &target) {
                Ok(()) => {
                    fs::File::open(&folder).and_then(|folder| folder.sync_all()).map_err(|error| format!("The copy was written but its save could not be confirmed: {error}"))?;
                    return Ok(target);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if bounded_file(&target, offer.bytes).is_ok_and(|bytes| offer.matches(&bytes)) {
                        fs::File::open(&target).and_then(|file| file.sync_all())
                            .and_then(|()| fs::File::open(&folder)?.sync_all())
                            .map_err(|error| format!("The copy is present, but its save could not be confirmed: {error}"))?;
                        return Ok(target);
                    }
                }
                Err(error) => return Err(format!("The copy could not be completed in this folder: {error}. Try a folder on your computer's local disk.")),
            }
        }
        Err("This folder already has many files with that title. Choose another folder.".into())
    })();
    drop(file);
    let _cleanup = fs::remove_file(&temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::exports::Format;

    #[test]
    fn receive_keeps_existing_files_and_reuses_a_complete_identical_copy() {
        let root =
            std::env::temp_dir().join(format!("cobalt-export-publish-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let bytes = b"A quiet place to read.\n";
        let offer = Offer {
            title: "../../Reading notes".into(),
            format: Format::Text,
            digest: kobo_net::sha256::hex_digest(bytes),
            bytes: bytes.len(),
        };
        fs::write(root.join("Reading notes.txt"), b"Keep my original").unwrap();
        let received = publish(&root, "fieldbook", &offer, bytes).unwrap();
        assert_eq!(received.file_name().unwrap(), "Reading notes (2).txt");
        assert_eq!(fs::read(&received).unwrap(), bytes);
        assert_eq!(
            fs::read(root.join("Reading notes.txt")).unwrap(),
            b"Keep my original"
        );
        assert_eq!(
            publish(&root, "fieldbook", &offer, bytes).unwrap(),
            received
        );
        assert!(publish(
            &root.join("should-not-exist"),
            "fieldbook",
            &offer,
            b"wrong"
        )
        .is_err());
        assert!(!root.join("should-not-exist").exists());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symlinks_and_overlong_sources_are_refused_before_receiving() {
        let root =
            std::env::temp_dir().join(format!("cobalt-export-source-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("original"), b"owner data").unwrap();
        std::os::unix::fs::symlink(root.join("original"), root.join("link")).unwrap();
        assert!(bounded_file(&root.join("link"), 100).is_err());
        assert!(bounded_file(&root.join("original"), 3).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn command_requires_a_single_explicit_destination_and_known_app_identity() {
        for arguments in [
            vec!["--app", "../secrets", "--sim", "--out", "/tmp"],
            vec![
                "--app",
                "todo",
                "--sim",
                "--device",
                "reader.local",
                "--out",
                "/tmp",
            ],
            vec![
                "--app",
                "todo",
                "--device",
                "reader.local;bad",
                "--out",
                "/tmp",
            ],
            vec!["--app", "todo", "--sim"],
        ] {
            assert!(
                command(&arguments.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
        let script = remote_script("todo", OFFER_KEY, MAX_OFFER_BYTES, false);
        assert!(script.contains("head -c 4097"));
        assert!(script.contains("test ! -L"));
        assert!(script.contains("/state/todo/cobalt-export"));
        assert!(!script.contains("rm "));
    }
}
