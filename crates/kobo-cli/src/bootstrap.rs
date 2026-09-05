//! Stable launch entrypoint outside the versioned installation transaction.

use std::fs;
use std::io::Write;
use std::path::Path;

pub const RELATIVE_PATH: &str = ".adds/cobalt-launch.sh";
pub const DEVICE_PATH: &str = "/mnt/onboard/.adds/cobalt-launch.sh";
pub(crate) const CONTENT: &str = include_str!("../../../assets/cobalt-launch.sh");
const NICKELMENU_CONFIGS: [&str; 2] = [".adds/nm/cobalt", ".adds/nm/menu"];
const OLD_DEVICE_PATH: &str = "/mnt/onboard/.adds/cobalt/start.sh";

/// Durably installs the entrypoint NickelMenu launches.
///
/// # Errors
///
/// When the mounted volume refuses the temporary write, sync, or rename, or a
/// NickelMenu path that would be migrated is not a regular file.
pub fn install(volume: &Path) -> Result<(), String> {
    let destination = volume.join(RELATIVE_PATH);
    let parent = destination
        .parent()
        .ok_or_else(|| "the launch entrypoint has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    atomic_write(&destination, CONTENT.as_bytes(), None, true)?;
    verify(&destination)?;
    migrate_nickelmenu(volume)
}

fn verify(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes != CONTENT.as_bytes() {
        return Err(format!("{} did not verify after writing", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("{} is not executable", path.display()));
        }
    }
    Ok(())
}

fn migrate_nickelmenu(volume: &Path) -> Result<(), String> {
    let directory = volume.join(".adds/nm");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "refusing non-directory NickelMenu path {}",
                directory.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect {}: {error}", directory.display())),
    }
    for relative in NICKELMENU_CONFIGS {
        migrate_nickelmenu_file(&volume.join(relative))?;
    }
    Ok(())
}

fn migrate_nickelmenu_file(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Err(format!(
                "refusing non-file NickelMenu path {}",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    let original =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let migrated = migrate_lines(&original);
    if migrated == original {
        return Ok(());
    }
    atomic_write(
        path,
        migrated.as_bytes(),
        Some(metadata.permissions()),
        false,
    )
}

fn migrate_lines(original: &str) -> String {
    let mut migrated = String::with_capacity(original.len());
    for line in original.split_inclusive('\n') {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let body = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        if body.split_whitespace().eq([
            "menu_item",
            ":main",
            ":Cobalt",
            ":cmd_spawn",
            ":quiet:/mnt/onboard/.adds/cobalt/start.sh",
        ]) {
            migrated.push_str(&line.replacen(OLD_DEVICE_PATH, DEVICE_PATH, 1));
        } else {
            migrated.push_str(line);
        }
    }
    migrated
}

fn remove_cobalt_lines(original: &str) -> String {
    let mut kept = String::with_capacity(original.len());
    for line in original.split_inclusive('\n') {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let body = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
        let mut tokens = body.split_whitespace();
        let exact_prefix = tokens.next() == Some("menu_item")
            && tokens.next() == Some(":main")
            && tokens.next() == Some(":Cobalt")
            && tokens.next() == Some(":cmd_spawn");
        let command = tokens.next();
        let exact_command = matches!(
            command,
            Some(
                ":quiet:/mnt/onboard/.adds/cobalt/start.sh"
                    | ":quiet:/mnt/onboard/.adds/cobalt-launch.sh"
            )
        );
        if !(exact_prefix && exact_command && tokens.next().is_none()) {
            kept.push_str(line);
        }
    }
    kept
}

fn atomic_write(
    destination: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    executable: bool,
) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", destination.display()))?;
    let temporary = destination.with_extension("new");
    match fs::symlink_metadata(&temporary) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(&temporary)
                .map_err(|error| format!("remove {}: {error}", temporary.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(format!(
                "refusing non-file temporary path {}",
                temporary.display()
            ));
        }
        Err(error) => return Err(format!("inspect {}: {error}", temporary.display())),
    }
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        } else if executable {
            set_executable(&temporary)?;
        }
        file.sync_all()?;
        fs::rename(&temporary, destination)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ignored = fs::remove_file(&temporary);
    }
    result.map_err(|error| format!("write {}: {error}", destination.display()))
}

/// Removes the stable entrypoint during setup undo.
pub fn remove(volume: &Path) -> Result<bool, String> {
    let path = volume.join(RELATIVE_PATH);
    let removed = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(&path)
                .map_err(|error| format!("remove {}: {error}", path.display()))?;
            sync_directory(
                path.parent()
                    .ok_or_else(|| format!("{} has no parent directory", path.display()))?,
            )
            .map_err(|error| format!("sync after removing {}: {error}", path.display()))?;
            true
        }
        Ok(_) => {
            return Err(format!(
                "refusing non-file bootstrap path {}",
                path.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    Ok(removed)
}

/// Removes only exact legacy/current Cobalt entries from both supported
/// NickelMenu files. The dedicated file is deleted only when nothing but
/// Cobalt's generated comments remains.
pub(crate) fn remove_menu_entries(volume: &Path) -> Result<bool, String> {
    let mut removed = false;
    let directory = volume.join(".adds/nm");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "refusing non-directory NickelMenu path {}",
                directory.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect {}: {error}", directory.display())),
    }
    for relative in NICKELMENU_CONFIGS {
        let config = volume.join(relative);
        let metadata = match fs::symlink_metadata(&config) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            Ok(_) => {
                return Err(format!(
                    "refusing non-file NickelMenu path {}",
                    config.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect {}: {error}", config.display())),
        };
        let original = fs::read_to_string(&config)
            .map_err(|error| format!("read {}: {error}", config.display()))?;
        let mut kept = remove_cobalt_lines(&original);
        if relative == ".adds/nm/cobalt" {
            kept = remove_generated_comments(&kept);
        }
        if kept != original {
            if relative == ".adds/nm/cobalt" && kept.trim().is_empty() {
                fs::remove_file(&config)
                    .map_err(|error| format!("remove {}: {error}", config.display()))?;
                sync_directory(&directory).map_err(|error| {
                    format!("sync after removing {}: {error}", config.display())
                })?;
            } else {
                atomic_write(
                    &config,
                    kept.as_bytes(),
                    Some(metadata.permissions()),
                    false,
                )?;
            }
            removed = true;
        }
    }
    Ok(removed)
}

fn remove_generated_comments(content: &str) -> String {
    content
        .split_inclusive('\n')
        .filter(|line| {
            !matches!(
                line.trim(),
                "#" | "# Cobalt. Written by 'kobo setup'; removed by 'kobo setup --undo'."
                    | "# Starting Cobalt stops the reader and takes over the screen. Restart"
                    | "# the device to get the reader back."
            )
        })
        .collect()
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join(format!("bootstrap-test-{}-{name}", std::process::id()));
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch root");
        root
    }

    #[test]
    fn installs_bootstrap_and_migrates_only_exact_entries_in_both_configs() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("exact");
        fs::create_dir_all(root.join(".adds/nm")).expect("NickelMenu folder");
        let exact = format!(
            "menu_item :main :Cobalt :cmd_spawn :quiet:{}",
            super::OLD_DEVICE_PATH
        );
        let unrelated = format!(
            "menu_item :main :Other :cmd_spawn :quiet:{} --extra",
            super::OLD_DEVICE_PATH
        );
        fs::write(
            root.join(".adds/nm/cobalt"),
            format!("header\n{exact}\n{unrelated}\n"),
        )
        .expect("legacy cobalt config");
        fs::write(root.join(".adds/nm/menu"), format!("{exact}\r\nfooter\r\n"))
            .expect("legacy menu config");

        super::install(&root).expect("install bootstrap");

        let path = root.join(super::RELATIVE_PATH);
        assert_eq!(
            fs::read_to_string(&path).expect("bootstrap"),
            super::CONTENT
        );
        #[cfg(unix)]
        assert_ne!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o111,
            0
        );
        assert_eq!(
            fs::read_to_string(root.join(".adds/nm/cobalt")).expect("cobalt config"),
            format!(
                "header\nmenu_item :main :Cobalt :cmd_spawn :quiet:{}\n{unrelated}\n",
                super::DEVICE_PATH
            )
        );
        assert_eq!(
            fs::read_to_string(root.join(".adds/nm/menu")).expect("menu config"),
            format!(
                "menu_item :main :Cobalt :cmd_spawn :quiet:{}\r\nfooter\r\n",
                super::DEVICE_PATH
            )
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn undo_removes_bootstrap_and_only_exact_cobalt_menu_lines() {
        let root = scratch("remove");
        fs::create_dir_all(root.join(".adds/nm")).expect("NickelMenu folder");
        fs::write(root.join(super::RELATIVE_PATH), super::CONTENT).expect("bootstrap");
        fs::write(
            root.join(".adds/nm/menu"),
            "before\r\nmenu_item :main :Cobalt :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt-launch.sh\r\nmenu_item :main :Other :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt-launch.sh\r\nafter\r\n",
        )
        .expect("shared menu");
        fs::write(
            root.join(".adds/nm/cobalt"),
            "menu_item :main :Cobalt :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt/start.sh\n",
        )
        .expect("legacy menu");

        assert!(super::remove(&root).expect("remove"));
        assert!(super::remove_menu_entries(&root).expect("remove menu entries"));

        assert!(!root.join(super::RELATIVE_PATH).exists());
        assert_eq!(
            fs::read_to_string(root.join(".adds/nm/menu")).expect("shared menu"),
            "before\r\nmenu_item :main :Other :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt-launch.sh\r\nafter\r\n"
        );
        assert!(!root.join(".adds/nm/cobalt").exists());
        let _ignored = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_nickelmenu_files_and_directories() {
        use std::os::unix::fs::symlink;

        let root = scratch("symlinks");
        fs::create_dir_all(root.join(".adds/nm")).expect("NickelMenu folder");
        fs::write(root.join("victim"), "unchanged").expect("victim");
        symlink(root.join("victim"), root.join(".adds/nm/menu")).expect("file symlink");
        assert!(super::install(&root).is_err());
        assert_eq!(
            fs::read_to_string(root.join("victim")).expect("victim"),
            "unchanged"
        );

        fs::remove_dir_all(root.join(".adds/nm")).expect("remove NickelMenu folder");
        symlink(root.join("victim"), root.join(".adds/nm")).expect("directory symlink");
        assert!(super::install(&root).is_err());
        assert_eq!(
            fs::read_to_string(root.join("victim")).expect("victim"),
            "unchanged"
        );
        let _ignored = fs::remove_dir_all(root);
    }
}
