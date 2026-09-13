//! Two bounded recovery slots; the pointer changes only after a complete copy.
use kobo_frame_host::{Manifest, MANIFEST};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const POINTER: &str = ".recovery-current";

fn slot(root: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(root.join(POINTER)) {
        Ok(value) if value == "a" || value == "b" => Ok(Some(value)),
        Ok(_) => Err("Frame recovery pointer is invalid".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Read Frame recovery: {error}")),
    }
}

pub fn save(root: &Path, previous: &Manifest) -> Result<(), String> {
    if previous.photos.is_empty() {
        return Ok(());
    }
    let next = if slot(root)?.as_deref() == Some("a") {
        "b"
    } else {
        "a"
    };
    let dest = root.join(format!(".recovery-{next}"));
    let result = (|| -> std::io::Result<()> {
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::create_dir(&dest)?;
        for photo in &previous.photos {
            fs::copy(root.join(photo.shelf_name()), dest.join(photo.shelf_name()))?;
        }
        fs::write(dest.join(MANIFEST), previous.encode())?;
        fs::write(root.join(".recovery-pointer.writing"), next)?;
        fs::rename(root.join(".recovery-pointer.writing"), root.join(POINTER))?;
        Ok(())
    })();
    result.map_err(|error| {
        format!("Could not preserve the previous Frame album; shelf unchanged: {error}")
    })
}

pub fn load(root: &Path) -> Result<(Manifest, std::path::PathBuf), String> {
    let current = slot(root)?.ok_or("No previous Frame album is available")?;
    let dir = root.join(format!(".recovery-{current}"));
    let bytes =
        fs::read(dir.join(MANIFEST)).map_err(|e| format!("Read previous Frame album: {e}"))?;
    let manifest = Manifest::decode(&bytes)?;
    for photo in &manifest.photos {
        if !dir.join(photo.shelf_name()).is_file() {
            return Err(format!(
                "Previous Frame photo {} is missing; shelf unchanged",
                photo.name
            ));
        }
    }
    Ok((manifest, dir))
}

pub fn restore(root: &Path) -> Result<usize, String> {
    let (manifest, dir) = load(root)?;
    let current = Manifest::decode(
        &fs::read(root.join(MANIFEST)).map_err(|e| format!("Read current Frame shelf: {e}"))?,
    )?;
    for photo in &manifest.photos {
        let partial = root.join(format!(".{}.restoring", photo.id));
        fs::copy(dir.join(photo.shelf_name()), &partial)
            .map_err(|e| format!("Restore Frame photo: {e}"))?;
        fs::rename(partial, root.join(photo.shelf_name()))
            .map_err(|e| format!("Publish restored Frame photo: {e}"))?;
    }
    let partial = root.join(".manifest.restoring");
    fs::write(&partial, manifest.encode()).map_err(|e| format!("Restore Frame manifest: {e}"))?;
    fs::rename(partial, root.join(MANIFEST))
        .map_err(|e| format!("Publish restored Frame manifest: {e}"))?;
    for photo in current.photos {
        if !manifest.photos.iter().any(|p| p.id == photo.id) {
            fs::remove_file(root.join(photo.shelf_name()))
                .map_err(|e| format!("Remove replaced Frame photo: {e}"))?;
        }
    }
    Ok(manifest.photos.len())
}

// Caller supplies a fixed, trusted root and manifest-decoded photo IDs.
pub fn save_script(previous: &Manifest) -> String {
    if previous.photos.is_empty() {
        return String::new();
    }
    let mut script = format!("current=\"\"\nif [ -f \"$root/{POINTER}\" ]; then current=$(cat \"$root/{POINTER}\"); fi\ncase \"$current\" in a) next=b ;; b|'') next=a ;; *) echo 'Invalid Frame recovery pointer' >&2; exit 1 ;; esac\nbackup=\"$root/.recovery-$next\"\nrm -rf \"$backup\"\nmkdir \"$backup\"\n");
    for photo in &previous.photos {
        let _ = writeln!(
            script,
            "cp \"$root/{}.png\" \"$backup/{}.png\"",
            photo.id, photo.id
        );
    }
    let _ = write!(script, "cp \"$root/{MANIFEST}\" \"$backup/{MANIFEST}\"\nsync\nprintf '%s' \"$next\" > \"$root/.recovery-pointer.writing\"\nmv -f \"$root/.recovery-pointer.writing\" \"$root/{POINTER}\"\nsync\n");
    script
}

pub fn select_script() -> String {
    format!("current=$(cat \"$root/{POINTER}\")\ncase \"$current\" in a|b) ;; *) echo 'No valid previous Frame album' >&2; exit 1 ;; esac\nbackup=\"$root/.recovery-$current\"\n")
}

pub fn restore_script(manifest: &Manifest) -> String {
    let mut script = select_script();
    // Check every file before touching the current shelf.
    for photo in &manifest.photos {
        let _ = writeln!(script, "test -f \"$backup/{}.png\"", photo.id);
    }
    for photo in &manifest.photos {
        let _ = writeln!(
            script,
            "cp \"$backup/{}.png\" \"$root/.{}.restoring\"\nmv -f \"$root/.{}.restoring\" \"$root/{}.png\"",
            photo.id, photo.id, photo.id, photo.id
        );
    }
    let _ = writeln!(
        script,
        "cp \"$backup/{MANIFEST}\" \"$root/.manifest.restoring\"\nmv -f \"$root/.manifest.restoring\" \"$root/{MANIFEST}\"\nsync"
    );
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_frame_host::Photo;

    #[test]
    fn remote_snapshot_and_restore_scripts_preserve_previous_album() {
        let root =
            std::env::temp_dir().join(format!("frame-recovery-shell-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let manifest = Manifest {
            photos: vec![Photo {
                id: "photo-0123456789abcdef".into(),
                digest: "0123456789abcdef".into(),
                taken: 0,
                album: "Original".into(),
                name: "photo.png".into(),
            }],
        };
        fs::write(root.join(MANIFEST), manifest.encode()).unwrap();
        fs::write(
            root.join(manifest.photos[0].shelf_name()),
            b"original image bytes",
        )
        .unwrap();
        let execute = |body: String| {
            let mut command = std::process::Command::new("sh");
            command
                .arg("-c")
                .arg(format!("set -eu\nroot=$1\n{body}"))
                .arg("frame-test")
                .arg(&root)
                .output()
                .unwrap()
        };
        assert!(execute(save_script(&manifest)).status.success());
        fs::write(root.join(MANIFEST), Manifest::default().encode()).unwrap();
        fs::remove_file(root.join(manifest.photos[0].shelf_name())).unwrap();
        assert!(execute(restore_script(&manifest)).status.success());
        assert_eq!(fs::read(root.join(MANIFEST)).unwrap(), manifest.encode());
        assert_eq!(
            fs::read(root.join(manifest.photos[0].shelf_name())).unwrap(),
            b"original image bytes"
        );
        // A failed new copy keeps the last complete recovery pointer.
        fs::remove_file(root.join(manifest.photos[0].shelf_name())).unwrap();
        assert!(!execute(save_script(&manifest)).status.success());
        assert_eq!(fs::read_to_string(root.join(POINTER)).unwrap(), "a");
        assert!(execute(restore_script(&manifest)).status.success());
        fs::remove_dir_all(root).unwrap();
    }
}
