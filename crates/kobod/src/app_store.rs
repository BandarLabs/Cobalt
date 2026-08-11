//! Runtime-owned public application catalog and atomic app transactions.

use kobo_app_store::{
    parse_public_bundle, verify, Catalog, DetachedSignature, Ed25519PublicKey, Manifest,
};
use kobo_protocol::{AppInfo, DeviceError};
use kobo_ui::Glyph;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CATALOG_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json";
pub const CATALOG_SIGNATURE_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json.sig";

const CATALOG_LIMIT: u32 = 512 * 1024;
const SIGNATURE_LIMIT: u32 = 1024;
const UNINSTALLED_SUFFIX: &str = "uninstalled";

struct BuiltinApp {
    id: &'static str,
    title: &'static str,
    label: &'static str,
    summary: &'static str,
    version: &'static str,
    glyph: Glyph,
    capabilities: &'static [&'static str],
}

const MANAGED_BUILTINS: &[BuiltinApp] = &[
    BuiltinApp {
        id: "audiobook",
        title: "Audiobook Studio",
        label: "Audiobooks",
        summary: "Research, narrate and play an original audiobook about any topic.",
        version: "1.0.0",
        glyph: Glyph::Headphones,
        // `bluetooth-control` is here because the player this application
        // opens is `kobo_sdk::audio::AudioPlayer`, and the Clara BW has no
        // speaker: when no audio device is connected, Play shows the
        // component's own picker, which scans, pairs and connects. Those are
        // `bluetooth-control` requests made on the application's behalf, so
        // declaring only `bluetooth-audio` left Play refused the moment a
        // reader had nothing paired -- which is every reader, the first time.
        capabilities: &["network", "audio", "bluetooth-audio", "bluetooth-control"],
    },
    BuiltinApp {
        id: "brief",
        title: "Daily Brief",
        label: "Daily Brief",
        summary: "Collects the day's stories while you read something else.",
        version: "1.0.0",
        glyph: Glyph::Clock,
        capabilities: &["network"],
    },
    BuiltinApp {
        id: "chat",
        title: "AI Command Center",
        label: "AI Chat",
        summary: "Ask a question and tap the answer, rather than typing one.",
        version: "1.0.0",
        glyph: Glyph::Chat,
        capabilities: &["network"],
    },
    BuiltinApp {
        id: "gallery",
        title: "Components",
        label: "Components",
        summary: "Every UI primitive on real hardware, for checking by eye.",
        version: "1.0.0",
        glyph: Glyph::Chart,
        capabilities: &["network"],
    },
    BuiltinApp {
        id: "gutenbird",
        title: "Gutenbird",
        label: "Gutenbird",
        summary: "Sixty thousand free books from Project Gutenberg.",
        version: "1.0.0",
        glyph: Glyph::Book,
        capabilities: &["network", "frontlight-control"],
    },
    BuiltinApp {
        id: "hn",
        title: "Hacker News",
        label: "Hacker News",
        summary: "Top, New, Ask and Show, with whole comment threads.",
        version: "1.0.0",
        glyph: Glyph::News,
        capabilities: &["network"],
    },
    BuiltinApp {
        id: "magnet",
        title: "Magnet",
        label: "Magnet",
        summary: "Find the hall sensor behind the bezel and watch it answer.",
        version: "1.0.0",
        glyph: Glyph::Magnet,
        capabilities: &["cover-sensor"],
    },
    BuiltinApp {
        id: "rss",
        title: "Feeds",
        label: "Feeds",
        summary: "Follow a site by name and read its articles, not its layout.",
        version: "1.0.0",
        glyph: Glyph::Rss,
        capabilities: &["network"],
    },
    BuiltinApp {
        id: "sidekick",
        title: "Sidekick",
        label: "Sidekick",
        summary: "Approve or deny what your coding agents ask to run, from here.",
        version: "1.0.0",
        glyph: Glyph::Key,
        capabilities: &["network"],
    },
    BuiltinApp {
        id: "tictactoe",
        title: "Tic-tac-toe",
        label: "Tic-tac-toe",
        summary: "Two players, one panel. Nought goes first.",
        version: "1.0.0",
        glyph: Glyph::Grid,
        capabilities: &[],
    },
    BuiltinApp {
        id: "todo",
        title: "Todo",
        label: "Todo",
        summary: "A list that remembers itself. Tap an item to finish it.",
        version: "1.0.0",
        glyph: Glyph::Check,
        capabilities: &[],
    },
];

const SYSTEM_APPS: &[BuiltinApp] = &[
    BuiltinApp {
        id: "settings",
        title: "Settings",
        label: "Settings",
        summary: "Connect Wi-Fi, manage hardware and update the Cobalt platform.",
        version: env!("CARGO_PKG_VERSION"),
        glyph: Glyph::Settings,
        capabilities: &[
            "network",
            "battery-read",
            "bluetooth-control",
            "wifi-control",
        ],
    },
    BuiltinApp {
        id: "terminal",
        title: "Terminal",
        label: "Terminal",
        summary: "A shell on the panel, with keys that send rather than collect.",
        version: env!("CARGO_PKG_VERSION"),
        glyph: Glyph::Terminal,
        capabilities: &["shell"],
    },
];

pub fn refresh(root: &Path) -> Result<Vec<AppInfo>, DeviceError> {
    let key = public_key()?;
    refresh_with(root, &key, |url, maximum| {
        kobo_net::fetch(url, maximum).map_err(network_error)
    })
}

pub fn catalog(root: &Path) -> Result<Vec<AppInfo>, DeviceError> {
    let key = public_key()?;
    match read_cached_catalog(root, &key) {
        Ok(catalog) => catalog_info(root, &catalog, &key),
        Err(DeviceError::NotFound) => local_catalog_info(root, &key),
        Err(error) => Err(error),
    }
}

pub fn installed(root: &Path) -> Result<Vec<AppInfo>, DeviceError> {
    let key = public_key()?;
    let mut entries = installed_manifests(root, &key)?
        .into_iter()
        .map(|manifest| manifest_info(&manifest, Some(manifest.version())))
        .collect::<Result<Vec<_>, _>>()?;
    let installed_ids = entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<BTreeSet<_>>();
    for app in MANAGED_BUILTINS {
        if !installed_ids.contains(app.id) && builtin_is_installed(root, app.id)? {
            entries.push(builtin_info(app));
        }
    }
    sort_info(&mut entries);
    Ok(entries)
}

pub fn install(root: &Path, id: &str) -> Result<(), DeviceError> {
    install_with(root, id, &public_key()?, |url, maximum| {
        kobo_net::fetch(url, maximum).map_err(network_error)
    })
}

#[must_use]
pub fn manages_builtin(id: &str) -> bool {
    managed_builtin(id).is_some()
}

pub fn builtin_declared(id: &str) -> Option<kobo_policy::Declared> {
    let app = managed_builtin(id)?;
    kobo_policy::Declared::parse(app.capabilities.iter().copied()).ok()
}

pub fn uninstall(root: &Path, id: &str) -> Result<(), DeviceError> {
    if !kobo_protocol::valid_app_id(id) || kobo_app_store::is_public_reserved_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    let key = public_key()?;
    recover_interrupted_transaction(root, id, &key)?;
    let apps = apps_root(root);
    let current = apps.join(id);
    let has_current = safe_directory(&current)?;
    let has_builtin = managed_builtin(id).is_some() && builtin_is_installed(root, id)?;
    if !has_current && !has_builtin {
        return Err(DeviceError::NotFound);
    }
    fs::create_dir_all(&apps).map_err(|_| DeviceError::Backend)?;
    sync_directory(root)?;
    remove_directory(&apps.join(format!("{id}.prev")))?;
    let removed = apps.join(format!("{id}.removed"));
    remove_directory(&removed)?;
    let tombstone = apps.join(format!("{id}.{UNINSTALLED_SUFFIX}"));
    remove_file(&tombstone)?;
    if has_current {
        rename_synced(&current, &removed, &apps)?;
    }
    if write_synced(&tombstone, b"committed\n")
        .and_then(|()| sync_directory(&apps))
        .is_err()
    {
        let _ignored = remove_file(&tombstone);
        if has_current {
            let _ignored = rename_synced(&removed, &current, &apps);
        }
        return Err(DeviceError::Backend);
    }
    // The absence of the current directory is now the durable commit. Cleanup
    // can be retried later and must never roll a partially deleted app back.
    let _ignored = remove_directory(&removed);
    Ok(())
}

pub fn resolve(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !kobo_protocol::valid_app_id(id) {
        return Err(format!("{id:?} is not a valid application identity"));
    }
    let key = public_key().map_err(|error| format!("read app signing key: {error:?}"))?;
    recover_interrupted_transaction(root, id, &key)
        .map_err(|error| format!("recover application {id}: {error:?}"))?;
    if is_uninstalled(root, id)
        .map_err(|error| format!("read application {id} state: {error:?}"))?
    {
        return Err(format!("no application named {id} is installed"));
    }
    let directory = apps_root(root).join(id);
    if safe_directory(&directory).map_err(|error| format!("read application {id}: {error:?}"))? {
        let manifest = read_installed_manifest(&directory, id, &key)
            .map_err(|error| format!("installed application {id} is invalid: {error:?}"))?;
        return Ok(app_binary(root, manifest.id()));
    }
    let builtin = builtin_binary(root, id);
    if managed_builtin(id).is_some() && builtin.is_file() {
        return Ok(builtin);
    }
    Err(format!("no application named {id} is installed"))
}

pub fn declared(root: &Path, id: &str) -> Option<kobo_policy::Declared> {
    let key = public_key().ok()?;
    installed_manifest(root, id, &key)
        .ok()
        .map(|manifest| manifest.declared_capabilities().clone())
}

fn refresh_with(
    root: &Path,
    key: &Ed25519PublicKey,
    mut fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<Vec<AppInfo>, DeviceError> {
    let json = fetch(CATALOG_URL, CATALOG_LIMIT)?;
    let signature = fetch(CATALOG_SIGNATURE_URL, SIGNATURE_LIMIT)?;
    let catalog = verify_catalog(&json, &signature, key)?;
    write_catalog_cache(root, &json, &signature)?;
    catalog_info(root, &catalog, key)
}

fn install_with(
    root: &Path,
    id: &str,
    key: &Ed25519PublicKey,
    mut fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<(), DeviceError> {
    if !kobo_protocol::valid_app_id(id) || kobo_app_store::is_public_reserved_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    recover_interrupted_transaction(root, id, key)?;
    let catalog = read_cached_catalog(root, key)?;
    let entry = catalog
        .entries()
        .iter()
        .find(|entry| entry.manifest().id() == id)
        .ok_or(DeviceError::NotFound)?;
    if !kobo_app_store::cobalt_version_at_least(
        env!("CARGO_PKG_VERSION"),
        entry.manifest().minimum_cobalt_version(),
    ) {
        return Err(DeviceError::InvalidInput);
    }
    let maximum = u32::try_from(entry.package_bytes()).map_err(|_| DeviceError::InvalidInput)?;
    let package = fetch(entry.package_url(), maximum)?;
    if package.len() as u64 != entry.package_bytes()
        || kobo_net::sha256::hex_digest(&package) != entry.package_sha256().as_str()
    {
        return Err(DeviceError::Integrity);
    }
    let bundle = parse_public_bundle(&package, key).map_err(|_| DeviceError::Integrity)?;
    if bundle.manifest() != entry.manifest() {
        return Err(DeviceError::Integrity);
    }
    stage_and_swap(root, bundle.manifest(), bundle.signature(), bundle.binary())
}

fn verify_catalog(
    json: &[u8],
    signature: &[u8],
    key: &Ed25519PublicKey,
) -> Result<Catalog, DeviceError> {
    let signature = std::str::from_utf8(signature)
        .map_err(|_| DeviceError::Integrity)?
        .trim_end_matches(['\r', '\n']);
    let signature = DetachedSignature::from_hex(signature).map_err(|_| DeviceError::Integrity)?;
    verify(json, &signature, key).map_err(|_| DeviceError::Integrity)?;
    let catalog = Catalog::parse_public(json).map_err(|_| DeviceError::InvalidInput)?;
    if catalog.to_canonical_bytes() != json {
        return Err(DeviceError::Integrity);
    }
    for entry in catalog.entries() {
        glyph(entry.manifest().glyph()).ok_or(DeviceError::InvalidInput)?;
    }
    Ok(catalog)
}

fn read_cached_catalog(root: &Path, key: &Ed25519PublicKey) -> Result<Catalog, DeviceError> {
    recover_catalog_cache(root, key)?;
    let cache = catalog_cache(root);
    read_catalog_directory(&cache, key)
}

fn read_catalog_directory(
    directory: &Path,
    key: &Ed25519PublicKey,
) -> Result<Catalog, DeviceError> {
    let json = fs::read(directory.join("catalog.json")).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DeviceError::NotFound
        } else {
            DeviceError::Backend
        }
    })?;
    let signature =
        fs::read(directory.join("catalog.json.sig")).map_err(|_| DeviceError::Integrity)?;
    verify_catalog(&json, &signature, key)
}

fn catalog_info(
    root: &Path,
    catalog: &Catalog,
    _key: &Ed25519PublicKey,
) -> Result<Vec<AppInfo>, DeviceError> {
    let installed = installed(root)?;
    let mut entries = catalog
        .entries()
        .iter()
        .map(|entry| {
            let version = installed
                .iter()
                .find(|installed| installed.id == entry.manifest().id())
                .and_then(|installed| installed.installed_version.as_deref());
            manifest_info(entry.manifest(), version)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for installed in installed {
        if !catalog
            .entries()
            .iter()
            .any(|entry| entry.manifest().id() == installed.id)
        {
            entries.push(installed);
        }
    }
    entries.extend(system_info(root)?);
    sort_info(&mut entries);
    Ok(entries)
}

fn local_catalog_info(root: &Path, _key: &Ed25519PublicKey) -> Result<Vec<AppInfo>, DeviceError> {
    let mut entries = installed(root)?;
    entries.extend(system_info(root)?);
    sort_info(&mut entries);
    Ok(entries)
}

fn system_info(root: &Path) -> Result<Vec<AppInfo>, DeviceError> {
    SYSTEM_APPS
        .iter()
        .filter_map(
            |app| match safe_regular_file(&builtin_binary(root, app.id)) {
                Ok(true) => Some(Ok(builtin_info(app))),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

fn builtin_info(app: &BuiltinApp) -> AppInfo {
    AppInfo {
        id: app.id.to_owned(),
        title: app.title.to_owned(),
        label: app.label.to_owned(),
        summary: app.summary.to_owned(),
        version: app.version.to_owned(),
        glyph: app.glyph,
        capabilities: app
            .capabilities
            .iter()
            .map(|capability| (*capability).to_owned())
            .collect(),
        installed_version: Some(app.version.to_owned()),
    }
}

fn managed_builtin(id: &str) -> Option<&'static BuiltinApp> {
    MANAGED_BUILTINS.iter().find(|app| app.id == id)
}

fn builtin_is_installed(root: &Path, id: &str) -> Result<bool, DeviceError> {
    Ok(!is_uninstalled(root, id)? && safe_regular_file(&builtin_binary(root, id))?)
}

fn is_uninstalled(root: &Path, id: &str) -> Result<bool, DeviceError> {
    safe_regular_file(&apps_root(root).join(format!("{id}.{UNINSTALLED_SUFFIX}")))
}

fn manifest_info(
    manifest: &Manifest,
    installed_version: Option<&str>,
) -> Result<AppInfo, DeviceError> {
    Ok(AppInfo {
        id: manifest.id().to_owned(),
        title: manifest.display_name().to_owned(),
        label: manifest.short_label().to_owned(),
        summary: manifest.summary().to_owned(),
        version: manifest.version().to_owned(),
        glyph: glyph(manifest.glyph()).ok_or(DeviceError::InvalidInput)?,
        capabilities: manifest.capabilities().map(str::to_owned).collect(),
        installed_version: installed_version.map(str::to_owned),
    })
}

fn sort_info(entries: &mut [AppInfo]) {
    entries.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn installed_manifests(root: &Path, key: &Ed25519PublicKey) -> Result<Vec<Manifest>, DeviceError> {
    let apps = apps_root(root);
    let directory = match fs::read_dir(&apps) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(DeviceError::Backend),
    };
    let mut ids = BTreeSet::new();
    for entry in directory {
        let entry = entry.map_err(|_| DeviceError::Backend)?;
        let name = entry.file_name();
        let Some(id) = name.to_str() else {
            continue;
        };
        if kobo_protocol::valid_app_id(id) {
            ids.insert(id.to_owned());
        } else if let Some(id) = id
            .strip_suffix(".removed")
            .or_else(|| id.strip_suffix(".prev"))
            .filter(|id| kobo_protocol::valid_app_id(id))
        {
            ids.insert(id.to_owned());
        }
    }
    let mut manifests = Vec::new();
    for id in ids {
        if let Ok(manifest) = installed_manifest(root, &id, key) {
            manifests.push(manifest);
        }
    }
    Ok(manifests)
}

fn installed_manifest(
    root: &Path,
    id: &str,
    key: &Ed25519PublicKey,
) -> Result<Manifest, DeviceError> {
    if !kobo_protocol::valid_app_id(id) || kobo_app_store::is_public_reserved_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    recover_interrupted_transaction(root, id, key)?;
    let directory = apps_root(root).join(id);
    if !safe_directory(&directory)? {
        return Err(DeviceError::NotFound);
    }
    read_installed_manifest(&directory, id, key)
}

fn read_installed_manifest(
    directory: &Path,
    id: &str,
    key: &Ed25519PublicKey,
) -> Result<Manifest, DeviceError> {
    let bytes = fs::read(directory.join("manifest.json")).map_err(|_| DeviceError::NotFound)?;
    let manifest = Manifest::parse_public(&bytes).map_err(|_| DeviceError::Integrity)?;
    if manifest.id() != id || manifest.to_canonical_bytes() != bytes {
        return Err(DeviceError::Integrity);
    }
    let signature = fs::read_to_string(directory.join("manifest.json.sig"))
        .map_err(|_| DeviceError::NotFound)?;
    let signature =
        DetachedSignature::from_hex(signature.trim()).map_err(|_| DeviceError::Integrity)?;
    verify(&bytes, &signature, key).map_err(|_| DeviceError::Integrity)?;
    let binary = fs::read(directory.join("bin").join(format!("kobo-{id}")))
        .map_err(|_| DeviceError::NotFound)?;
    if binary.len() as u64 != manifest.binary_bytes()
        || kobo_net::sha256::hex_digest(&binary) != manifest.binary_sha256().as_str()
    {
        return Err(DeviceError::Integrity);
    }
    Ok(manifest)
}

fn recover_interrupted_transaction(
    root: &Path,
    id: &str,
    key: &Ed25519PublicKey,
) -> Result<(), DeviceError> {
    let apps = apps_root(root);
    let current = apps.join(id);
    let removed = apps.join(format!("{id}.removed"));
    let tombstone = apps.join(format!("{id}.{UNINSTALLED_SUFFIX}"));
    if safe_regular_file(&tombstone)? {
        if safe_directory(&current)? && read_installed_manifest(&current, id, key).is_ok() {
            remove_file(&tombstone)?;
            remove_directory(&removed)?;
            return Ok(());
        }
        remove_directory(&current)?;
        remove_directory(&removed)?;
        remove_directory(&apps.join(format!("{id}.prev")))?;
        return Ok(());
    }
    if safe_directory(&current)? && read_installed_manifest(&current, id, key).is_ok() {
        return Ok(());
    }
    for suffix in ["removed", "prev"] {
        let candidate = apps.join(format!("{id}.{suffix}"));
        if safe_directory(&candidate)? && read_installed_manifest(&candidate, id, key).is_ok() {
            remove_directory(&current)?;
            rename_synced(&candidate, &current, &apps)?;
            for stale_suffix in ["removed", "prev"] {
                remove_directory(&apps.join(format!("{id}.{stale_suffix}")))?;
            }
            return Ok(());
        }
    }
    Ok(())
}

fn stage_and_swap(
    root: &Path,
    manifest: &Manifest,
    signature: DetachedSignature,
    binary: &[u8],
) -> Result<(), DeviceError> {
    let apps = apps_root(root);
    fs::create_dir_all(&apps).map_err(|_| DeviceError::Backend)?;
    sync_directory(root)?;
    let current = apps.join(manifest.id());
    let staging = apps.join(format!("{}.next", manifest.id()));
    let previous = apps.join(format!("{}.prev", manifest.id()));
    remove_directory(&staging)?;
    fs::create_dir_all(staging.join("bin")).map_err(|_| DeviceError::Backend)?;
    write_synced(
        &staging.join("manifest.json"),
        &manifest.to_canonical_bytes(),
    )?;
    write_synced(
        &staging.join("manifest.json.sig"),
        format!("{signature}\n").as_bytes(),
    )?;
    let binary_path = staging.join("bin").join(format!("kobo-{}", manifest.id()));
    write_synced(&binary_path, binary)?;
    set_executable(&binary_path)?;
    sync_file(&binary_path)?;
    sync_directory(&staging.join("bin"))?;
    sync_directory(&staging)?;
    sync_directory(&apps)?;
    remove_directory(&previous)?;
    let retired = if safe_directory(&current)? {
        rename_synced(&current, &previous, &apps)?;
        true
    } else {
        false
    };
    if fs::rename(&staging, &current).is_err() {
        if retired {
            let _ignored = rename_synced(&previous, &current, &apps);
        }
        let _ignored = fs::remove_dir_all(&staging);
        return Err(DeviceError::Backend);
    }
    sync_directory(&apps)?;
    remove_file(&apps.join(format!("{}.{UNINSTALLED_SUFFIX}", manifest.id())))?;
    Ok(())
}

fn write_catalog_cache(root: &Path, json: &[u8], signature: &[u8]) -> Result<(), DeviceError> {
    let store = cache_root(root);
    fs::create_dir_all(&store).map_err(|_| DeviceError::Backend)?;
    sync_directory(root)?;
    let current = catalog_cache(root);
    let next = store.join("catalog.next");
    let previous = store.join("catalog.prev");
    remove_directory(&next)?;
    fs::create_dir(&next).map_err(|_| DeviceError::Backend)?;
    if write_synced(&next.join("catalog.json"), json).is_err()
        || write_synced(&next.join("catalog.json.sig"), signature).is_err()
        || sync_directory(&next).is_err()
        || sync_directory(&store).is_err()
    {
        let _ignored = fs::remove_dir_all(&next);
        return Err(DeviceError::Backend);
    }
    remove_directory(&previous)?;
    let retired = if safe_directory(&current)? {
        rename_synced(&current, &previous, &store)?;
        true
    } else {
        false
    };
    if fs::rename(&next, &current).is_err() {
        if retired {
            let _ignored = rename_synced(&previous, &current, &store);
        }
        return Err(DeviceError::Backend);
    }
    sync_directory(&store)?;
    Ok(())
}

fn recover_catalog_cache(root: &Path, key: &Ed25519PublicKey) -> Result<(), DeviceError> {
    let current = catalog_cache(root);
    if safe_directory(&current)? && read_catalog_directory(&current, key).is_ok() {
        return Ok(());
    }
    let previous = cache_root(root).join("catalog.prev");
    if safe_directory(&previous)? && read_catalog_directory(&previous, key).is_ok() {
        remove_directory(&current)?;
        rename_synced(&previous, &current, &cache_root(root))?;
    }
    Ok(())
}

fn safe_directory(path: &Path) -> Result<bool, DeviceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_dir() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(DeviceError::Backend),
    }
}

fn safe_regular_file(path: &Path) -> Result<bool, DeviceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(DeviceError::Backend),
    }
}

fn remove_directory(path: &Path) -> Result<(), DeviceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).map_err(|_| DeviceError::Backend)?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(DeviceError::Backend),
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<(), DeviceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| DeviceError::Backend)?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(DeviceError::Backend),
    }
    Ok(())
}

fn write_synced(path: &Path, contents: &[u8]) -> Result<(), DeviceError> {
    let mut file = fs::File::create(path).map_err(|_| DeviceError::Backend)?;
    file.write_all(contents).map_err(|_| DeviceError::Backend)?;
    file.sync_all().map_err(|_| DeviceError::Backend)
}

fn sync_file(path: &Path) -> Result<(), DeviceError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| DeviceError::Backend)
}

fn sync_directory(path: &Path) -> Result<(), DeviceError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DeviceError::Backend)
}

fn rename_synced(source: &Path, destination: &Path, parent: &Path) -> Result<(), DeviceError> {
    fs::rename(source, destination).map_err(|_| DeviceError::Backend)?;
    sync_directory(parent)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), DeviceError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|_| DeviceError::Backend)?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|_| DeviceError::Backend)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), DeviceError> {
    Ok(())
}

fn public_key() -> Result<Ed25519PublicKey, DeviceError> {
    Ed25519PublicKey::from_hex(kobo_app_store::PUBLIC_RELEASE_KEY_HEX)
        .map_err(|_| DeviceError::Backend)
}

fn apps_root(root: &Path) -> PathBuf {
    root.join("apps")
}

fn cache_root(root: &Path) -> PathBuf {
    root.join("store")
}

fn catalog_cache(root: &Path) -> PathBuf {
    cache_root(root).join("catalog")
}

fn app_binary(root: &Path, id: &str) -> PathBuf {
    apps_root(root)
        .join(id)
        .join("bin")
        .join(format!("kobo-{id}"))
}

fn builtin_binary(root: &Path, id: &str) -> PathBuf {
    root.join("bin").join(format!("kobo-{id}"))
}

fn network_error(error: kobo_protocol::TaskError) -> DeviceError {
    match error {
        kobo_protocol::TaskError::Offline | kobo_protocol::TaskError::Unreachable => {
            DeviceError::Unreachable
        }
        kobo_protocol::TaskError::TimedOut => DeviceError::TimedOut,
        kobo_protocol::TaskError::NotFound => DeviceError::NotFound,
        kobo_protocol::TaskError::TooLarge | kobo_protocol::TaskError::Denied => {
            DeviceError::InvalidInput
        }
        kobo_protocol::TaskError::NoCredential | kobo_protocol::TaskError::Unauthorized => {
            DeviceError::Authentication
        }
    }
}

fn glyph(name: &str) -> Option<Glyph> {
    Some(match name {
        "app" => Glyph::App,
        "book" => Glyph::Book,
        "note" => Glyph::Note,
        "clock" => Glyph::Clock,
        "settings" => Glyph::Settings,
        "folder" => Glyph::Folder,
        "chart" => Glyph::Chart,
        "search" => Glyph::Search,
        "wifi" => Glyph::Wifi,
        "battery" => Glyph::Battery,
        "reader" => Glyph::Reader,
        "power" => Glyph::Power,
        "grid" => Glyph::Grid,
        "circle" => Glyph::Circle,
        "check" => Glyph::Check,
        "terminal" => Glyph::Terminal,
        "chat" => Glyph::Chat,
        "news" => Glyph::News,
        "rss" => Glyph::Rss,
        "light" => Glyph::Light,
        "close" => Glyph::Close,
        "download" => Glyph::Download,
        "bookmark" => Glyph::Bookmark,
        "filter" => Glyph::Filter,
        "person" => Glyph::Person,
        "tag" => Glyph::Tag,
        "globe" => Glyph::Globe,
        "refresh" => Glyph::Refresh,
        "more" => Glyph::More,
        "bluetooth" => Glyph::Bluetooth,
        "key" => Glyph::Key,
        "magnet" => Glyph::Magnet,
        "play" => Glyph::Play,
        "pause" => Glyph::Pause,
        "rewind30" => Glyph::Rewind30,
        "forward30" => Glyph::Forward30,
        "volume-down" => Glyph::VolumeDown,
        "volume-up" => Glyph::VolumeUp,
        "more-vertical" => Glyph::MoreVertical,
        "trash" => Glyph::Trash,
        "previous" => Glyph::Previous,
        "next" => Glyph::Next,
        "plus" => Glyph::Plus,
        "headphones" => Glyph::Headphones,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_app_store::{
        build_bundle, derive_public_key, sign, CatalogEntry, CatalogEntryInput, ManifestInput,
    };

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cobalt-app-store-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ignored = fs::remove_dir_all(&root);
        root
    }

    fn release(seed: &[u8; 32]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        release_for(seed, "word-count", "1.0.0")
    }

    fn release_for(seed: &[u8; 32], id: &str, version: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let binary = format!("{id} app binary").into_bytes();
        let manifest = Manifest::new_public(ManifestInput {
            id: id.to_owned(),
            display_name: format!("{id} application"),
            short_label: id.to_owned(),
            summary: format!("The {id} test application."),
            version: version.to_owned(),
            minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
            glyph: "note".to_owned(),
            capabilities: Vec::new(),
            binary_sha256: kobo_net::sha256::hex_digest(&binary),
            binary_bytes: binary.len() as u64,
        })
        .expect("manifest");
        let package = build_bundle(&manifest, &binary, seed).expect("bundle");
        let catalog = Catalog::new(vec![CatalogEntry::new(CatalogEntryInput {
            manifest,
            package_url: format!("https://example.test/{id}.cobalt-app"),
            package_sha256: kobo_net::sha256::hex_digest(&package),
            package_bytes: package.len() as u64,
        })
        .expect("entry")])
        .expect("catalog");
        let json = catalog.to_canonical_bytes();
        let signature = format!("{}\n", sign(&json, seed).expect("signature")).into_bytes();
        (json, signature, package)
    }

    #[test]
    fn bundled_apps_update_uninstall_and_reinstall_in_place() {
        let root = root();
        fs::create_dir_all(root.join("bin")).expect("built-in directory");
        fs::write(builtin_binary(&root, "todo"), b"built-in todo").expect("built-in Todo");

        let initial = installed(&root).expect("initial installed apps");
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].id, "todo");
        assert_eq!(initial[0].installed_version.as_deref(), Some("1.0.0"));
        assert_eq!(
            resolve(&root, "todo").expect("resolve built-in"),
            builtin_binary(&root, "todo")
        );

        uninstall(&root, "todo").expect("uninstall built-in");
        assert!(installed(&root).expect("removed apps").is_empty());
        assert!(resolve(&root, "todo").is_err());

        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release_for(&seed, "todo", "1.1.0");
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("refresh update");
        install_with(&root, "todo", &key, |_, _| Ok(package.clone())).expect("reinstall");
        let installed = installed_manifests(&root, &key).expect("reinstalled manifest");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version(), "1.1.0");
        assert!(!is_uninstalled(&root, "todo").expect("tombstone state"));

        let (json, signature, package) = release_for(&seed, "todo", "1.2.0");
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("refresh second update");
        install_with(&root, "todo", &key, |_, _| Ok(package.clone())).expect("update in place");
        let installed = installed_manifests(&root, &key).expect("updated manifest");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].version(), "1.2.0");
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn bundled_apps_use_the_same_capabilities_as_their_store_manifests() {
        assert!(builtin_declared("todo")
            .expect("Todo declaration")
            .is_empty());
        let audiobook = builtin_declared("audiobook").expect("Audiobook declaration");
        assert!(audiobook.holds(kobo_policy::Capability::Network));
        assert!(audiobook.holds(kobo_policy::Capability::Audio));
        assert!(audiobook.holds(kobo_policy::Capability::BluetoothAudio));
        // The player is `kobo_sdk::audio::AudioPlayer`, and on a device with
        // no speaker its picker scans, pairs and connects when nothing is
        // connected yet. Those are `bluetooth-control` requests, so an
        // application that shows that player and does not declare it has a
        // Play button that is refused for every reader who has not already
        // paired something -- which is every reader, once.
        assert!(audiobook.holds(kobo_policy::Capability::BluetoothControl));
        assert!(builtin_declared("terminal").is_none());
    }

    #[test]
    fn refresh_install_list_resolve_and_uninstall_are_complete() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release(&seed);
        let fetched = |url: &str, _: u32| match url {
            CATALOG_URL => Ok(json.clone()),
            CATALOG_SIGNATURE_URL => Ok(signature.clone()),
            "https://example.test/word-count.cobalt-app" => Ok(package.clone()),
            _ => Err(DeviceError::NotFound),
        };
        let listed = refresh_with(&root, &key, fetched).expect("refresh");
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].is_installed());
        install_with(&root, "word-count", &key, |url, _| {
            if url.ends_with(".cobalt-app") {
                Ok(package.clone())
            } else {
                Err(DeviceError::NotFound)
            }
        })
        .expect("install");
        assert_eq!(
            installed_manifests(&root, &key).expect("installed").len(),
            1
        );
        assert!(app_binary(&root, "word-count").is_file());
        uninstall(&root, "word-count").expect("uninstall");
        assert!(installed_manifests(&root, &key)
            .expect("installed")
            .is_empty());
        install_with(&root, "word-count", &key, |_, _| Ok(package.clone())).expect("reinstall");
        assert_eq!(
            installed_manifests(&root, &key).expect("reinstalled").len(),
            1
        );
        assert!(!apps_root(&root)
            .join(format!("word-count.{UNINSTALLED_SUFFIX}"))
            .exists());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_refresh_never_replaces_the_last_verified_catalog() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, _) = release(&seed);
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("first refresh");
        assert_eq!(
            refresh_with(&root, &key, |url, _| {
                if url == CATALOG_URL {
                    Ok(json.clone())
                } else {
                    Ok(b"0".repeat(128))
                }
            }),
            Err(DeviceError::Integrity)
        );
        assert_eq!(
            catalog_info(
                &root,
                &read_cached_catalog(&root, &key).expect("cache"),
                &key,
            )
            .expect("listing")
            .len(),
            1
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_install_and_uninstall_renames_recover_the_working_copy() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release(&seed);
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("refresh");
        install_with(&root, "word-count", &key, |_, _| Ok(package.clone())).expect("install");

        let apps = apps_root(&root);
        fs::rename(apps.join("word-count"), apps.join("word-count.prev"))
            .expect("interrupt install");
        assert_eq!(
            installed_manifests(&root, &key)
                .expect("recover install")
                .len(),
            1
        );
        assert!(apps.join("word-count").is_dir());

        fs::rename(apps.join("word-count"), apps.join("word-count.removed"))
            .expect("interrupt uninstall");
        assert_eq!(
            installed_manifests(&root, &key)
                .expect("recover uninstall")
                .len(),
            1
        );
        assert!(apps.join("word-count").is_dir());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn installed_manifest_signature_and_binary_are_verified_every_time() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release(&seed);
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("refresh");
        let install = || {
            install_with(&root, "word-count", &key, |_, _| Ok(package.clone())).expect("install");
        };

        install();
        let directory = apps_root(&root).join("word-count");
        let manifest_path = directory.join("manifest.json");
        let mut manifest = fs::read(&manifest_path).expect("manifest");
        let version = manifest
            .windows(5)
            .position(|window| window == b"1.0.0")
            .expect("version");
        manifest[version + 4] = b'1';
        fs::write(&manifest_path, manifest).expect("tamper manifest");
        assert_eq!(
            read_installed_manifest(&directory, "word-count", &key),
            Err(DeviceError::Integrity)
        );

        install();
        let signature_path = directory.join("manifest.json.sig");
        let mut manifest_signature = fs::read(&signature_path).expect("signature");
        manifest_signature[0] = if manifest_signature[0] == b'0' {
            b'1'
        } else {
            b'0'
        };
        fs::write(&signature_path, manifest_signature).expect("tamper signature");
        assert_eq!(
            read_installed_manifest(&directory, "word-count", &key),
            Err(DeviceError::Integrity)
        );

        install();
        let binary_path = app_binary(&root, "word-count");
        let mut binary = fs::read(&binary_path).expect("binary");
        binary[0] ^= 1;
        fs::write(binary_path, binary).expect("tamper binary");
        assert_eq!(
            read_installed_manifest(&directory, "word-count", &key),
            Err(DeviceError::Integrity)
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_current_app_recovers_only_from_a_verified_previous_copy() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release(&seed);
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("refresh");
        for _ in 0..2 {
            install_with(&root, "word-count", &key, |_, _| Ok(package.clone())).expect("install");
        }
        let apps = apps_root(&root);
        fs::write(
            apps.join("word-count").join("manifest.json.sig"),
            b"invalid",
        )
        .expect("corrupt current");
        assert_eq!(
            installed_manifest(&root, "word-count", &key)
                .expect("recover")
                .version(),
            "1.0.0"
        );
        assert!(!apps.join("word-count.prev").exists());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_catalog_write_recovers_the_last_verified_pair() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, _) = release(&seed);
        write_catalog_cache(&root, &json, &signature).expect("cache");
        let store = cache_root(&root);
        fs::rename(catalog_cache(&root), store.join("catalog.prev")).expect("retire cache");
        fs::create_dir(store.join("catalog.next")).expect("stage cache");
        fs::write(store.join("catalog.next/catalog.json"), b"incomplete").expect("partial cache");

        assert_eq!(
            read_cached_catalog(&root, &key)
                .expect("recover cache")
                .entries()
                .len(),
            1
        );
        assert!(catalog_cache(&root).join("catalog.json.sig").is_file());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn committed_uninstall_never_restores_a_retired_copy() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release(&seed);
        refresh_with(&root, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("refresh");
        for _ in 0..2 {
            install_with(&root, "word-count", &key, |_, _| Ok(package.clone())).expect("install");
        }
        let apps = apps_root(&root);
        remove_directory(&apps.join("word-count.prev")).expect("retire previous");
        fs::rename(apps.join("word-count"), apps.join("word-count.removed"))
            .expect("begin uninstall");
        fs::write(
            apps.join(format!("word-count.{UNINSTALLED_SUFFIX}")),
            b"committed\n",
        )
        .expect("commit uninstall");

        assert!(installed_manifests(&root, &key)
            .expect("recover committed uninstall")
            .is_empty());
        assert!(!apps.join("word-count").exists());
        assert!(!apps.join("word-count.prev").exists());
        assert!(apps
            .join(format!("word-count.{UNINSTALLED_SUFFIX}"))
            .is_file());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_glyphs_and_incompatible_versions_are_refused() {
        assert_eq!(glyph("not-a-glyph"), None);
        assert!(kobo_app_store::cobalt_version_at_least("0.2.0", "0.1.9"));
        assert!(!kobo_app_store::cobalt_version_at_least("0.1.8", "0.1.9"));
        assert!(!kobo_app_store::cobalt_version_at_least("nightly", "0.1.9"));
    }
}
