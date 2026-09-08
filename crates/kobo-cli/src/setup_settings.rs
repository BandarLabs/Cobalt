//! Preserve the owner's settings before USB setup changes them.
use super::{clear_setting, names_key, set_setting, INSTALL_FOLDER, SETTINGS, SETTINGS_APPLIED};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

const RECORD: &str = "state/setup-settings-v1.json";
const MAX_SETTINGS: u64 = 1024 * 1024;
const MAX_RECORD: u64 = 16 * 1024;

fn read(path: &Path, maximum: u64) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() <= maximum => {}
        _ => {
            return Err(format!(
                "Cannot safely read {}. The existing file was kept.",
                path.display()
            ))
        }
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("The settings file is too large. It was kept.".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "The settings file could not be read. It was kept.".into())
}

fn flush(directory: &Path) -> Result<(), String> {
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("Could not finish saving {}: {error}", directory.display()))
}

fn directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if !parent.exists() {
            directory(parent)?;
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| error.to_string())?;
        }
        _ => return Err(format!("{} is not a settings folder.", path.display())),
    }
    flush(path)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        flush(parent)?;
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing settings folder")?;
    directory(parent)?;
    let temporary = path.with_extension("cobalt-settings-new");
    match fs::symlink_metadata(&temporary) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::remove_file(&temporary).map_err(|error| error.to_string())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(
                "The settings recovery file needs attention. Existing settings were kept.".into(),
            )
        }
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    flush(parent)
}

fn value(text: &str, section: &str, key: &str) -> Result<Option<String>, String> {
    let mut inside = false;
    let mut found = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            inside = trimmed == format!("[{section}]");
        } else if inside && names_key(line, key) {
            if found.is_some() {
                return Err(format!(
                    "{section}/{key} appears more than once. Existing settings were kept."
                ));
            }
            found = line.split_once('=').map(|(_, value)| value.to_owned());
        }
    }
    Ok(found)
}

fn decode(text: &str) -> Result<Vec<Option<String>>, String> {
    let invalid = || {
        "The original-settings record could not be read. Existing settings were kept.".to_owned()
    };
    let data: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    if data.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(invalid());
    }
    let rows = data
        .get("settings")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if rows.len() != SETTINGS_APPLIED.len() {
        return Err(invalid());
    }
    rows.iter()
        .zip(SETTINGS_APPLIED)
        .map(|(row, (section, key, applied))| {
            if row.get("section").and_then(Value::as_str) != Some(section)
                || row.get("key").and_then(Value::as_str) != Some(key)
                || row.get("applied").and_then(Value::as_str) != Some(applied)
            {
                return Err(invalid());
            }
            match row.get("previous") {
                Some(Value::Null) => Ok(None),
                Some(Value::String(value))
                    if value.len() <= 2048 && !value.contains(['\r', '\n']) =>
                {
                    Ok(Some(value.clone()))
                }
                _ => Err(invalid()),
            }
        })
        .collect()
}

pub(super) fn edit(volume: &Path, apply: bool) -> Result<Vec<String>, String> {
    let path = volume.join(SETTINGS);
    let record = volume.join(INSTALL_FOLDER).join(RECORD);
    let original = read(&path, MAX_SETTINGS)?.unwrap_or_default();
    let current = SETTINGS_APPLIED
        .iter()
        .map(|(section, key, _)| value(&original, section, key))
        .collect::<Result<Vec<_>, _>>()?;
    let saved = read(&record, MAX_RECORD)?;
    let previous = match saved {
        Some(ref text) => decode(text)?,
        None if !apply => return Ok(Vec::new()), // Older installations have no trustworthy original values.
        None => {
            let rows: Vec<_> = SETTINGS_APPLIED.iter().zip(&current).map(|((section, key, applied), previous)| json!({"section": section, "key": key, "applied": applied, "previous": previous})).collect();
            let text = json!({"version": 1, "settings": rows}).to_string();
            decode(&text)?; // Refuse unrepresentable originals before making any change.
            write(&record, &text)?; // Must be durable before touching the reader's settings.
            current.clone()
        }
    };
    let mut edited = original.clone();
    let mut changed = Vec::new();
    for (((section, key, applied), current), previous) in
        SETTINGS_APPLIED.iter().zip(&current).zip(previous)
    {
        let next = if apply {
            if current.as_deref() == Some(applied) {
                continue;
            }
            set_setting(&edited, section, key, applied)
        } else {
            // Preserve any subsequent change made in the reader's settings.
            if current.as_deref() != Some(applied) {
                continue;
            }
            match previous {
                Some(previous) => set_setting(&edited, section, key, &previous),
                None => clear_setting(&edited, section, key),
            }
        };
        if next != edited {
            changed.push(format!("{section}/{key}"));
            edited = next;
        }
    }
    if edited != original {
        write(&path, &edited)?;
    }
    if !apply {
        fs::remove_file(&record).map_err(|error| error.to_string())?;
        flush(record.parent().ok_or("Missing settings record folder")?)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn volume(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("cobalt-settings-{name}-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".kobo/Kobo")).unwrap();
        root
    }
    #[test]
    fn repeated_setup_restores_original_values_and_keeps_other_changes() {
        let root = volume("repeat");
        fs::write(root.join(SETTINGS), "[DeveloperSettings]\nForceWifiOn=false\n[PowerOptions]\nAutoSleepMinutes=5\nFrontLightLevel=7\n").unwrap();
        assert_eq!(edit(&root, true).unwrap().len(), 2);
        let record = fs::read(root.join(INSTALL_FOLDER).join(RECORD)).unwrap();
        assert!(edit(&root, true).unwrap().is_empty());
        assert_eq!(
            fs::read(root.join(INSTALL_FOLDER).join(RECORD)).unwrap(),
            record
        );
        let now = fs::read_to_string(root.join(SETTINGS))
            .unwrap()
            .replace("FrontLightLevel=7", "FrontLightLevel=12");
        fs::write(root.join(SETTINGS), now).unwrap();
        assert_eq!(edit(&root, false).unwrap().len(), 2);
        let restored = fs::read_to_string(root.join(SETTINGS)).unwrap();
        assert!(restored.contains("ForceWifiOn=false\n"));
        assert!(restored.contains("AutoSleepMinutes=5\n"));
        assert!(restored.contains("FrontLightLevel=12\n"));
        assert!(edit(&root, false).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn undo_preserves_owner_changes_and_does_not_guess_legacy_values() {
        let root = volume("owner");
        fs::write(root.join(SETTINGS), "[PowerOptions]\nAutoSleepMinutes=90\n").unwrap();
        assert!(edit(&root, false).unwrap().is_empty());
        edit(&root, true).unwrap();
        let now = fs::read_to_string(root.join(SETTINGS))
            .unwrap()
            .replace("AutoSleepMinutes=90", "AutoSleepMinutes=15");
        fs::write(root.join(SETTINGS), now).unwrap();
        edit(&root, false).unwrap();
        let restored = fs::read_to_string(root.join(SETTINGS)).unwrap();
        assert!(restored.contains("AutoSleepMinutes=15\n"));
        assert!(!restored.contains("ForceWifiOn"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn damaged_backup_or_failed_backup_write_never_changes_settings() {
        let root = volume("failure");
        let original = "[PowerOptions]\nAutoSleepMinutes=5\n";
        fs::write(root.join(SETTINGS), original).unwrap();
        fs::create_dir_all(root.join(INSTALL_FOLDER).join("state")).unwrap();
        let record = root.join(INSTALL_FOLDER).join(RECORD);
        fs::write(&record, "{\"version\":2}").unwrap();
        assert!(edit(&root, true).is_err());
        assert!(edit(&root, false).is_err());
        fs::remove_file(&record).unwrap();
        fs::create_dir(record.with_extension("cobalt-settings-new")).unwrap();
        assert!(edit(&root, true).is_err());
        assert_eq!(fs::read_to_string(root.join(SETTINGS)).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }
}
