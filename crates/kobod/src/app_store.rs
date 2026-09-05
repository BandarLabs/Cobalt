//! Runtime-owned public application catalog and atomic app transactions.
#![allow(dead_code)]

use kobo_app_store::{
    parse_public_bundle, verify, Catalog, DetachedSignature, Ed25519PublicKey, Manifest,
};
use kobo_protocol::{AppInfo, DeviceError, RemoteInstallOutcome, UpdateChannel};
use kobo_ui::Glyph;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CATALOG_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json";
pub const CATALOG_SIGNATURE_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json.sig";
pub const BETA_CATALOG_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog-beta/cobalt-app-catalog.json";
pub const BETA_CATALOG_SIGNATURE_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog-beta/cobalt-app-catalog.json.sig";

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

/// Downloads, verifies, and atomically caches one channel's catalog.
///
/// # Errors
///
/// Returns a bounded device error when fetching, verification, parsing, or
/// cache activation fails.
pub fn refresh(root: &Path, channel: UpdateChannel) -> Result<Vec<AppInfo>, DeviceError> {
    let key = public_key()?;
    refresh_channel_with(root, channel, &key, |url, maximum| {
        kobo_net::fetch(url, maximum).map_err(network_error)
    })
}

/// Reads the last verified catalog for one channel.
///
/// # Errors
///
/// Returns a bounded device error when the cache or an installed package is
/// absent, malformed, or cannot be read.
pub fn catalog(root: &Path, channel: UpdateChannel) -> Result<Vec<AppInfo>, DeviceError> {
    let key = public_key()?;
    match read_channel_catalog(root, channel, &key) {
        Ok(catalog) => catalog_info(root, &catalog, &key),
        Err(DeviceError::NotFound) => local_catalog_info(root, &key),
        Err(error) => Err(error),
    }
}

/// Lists every verified installed Store application.
///
/// # Errors
///
/// Returns a bounded device error when installed metadata or package bytes
/// cannot be verified.
pub fn installed(root: &Path) -> Result<Vec<AppInfo>, DeviceError> {
    let key = public_key()?;
    installed_with_key(root, &key)
}

/// Lists installed applications using an explicitly supplied trust key.
///
/// This is intended for isolated host fixtures. Device code should use
/// [`installed`], which always selects Cobalt's built-in release key.
///
/// # Errors
///
/// Returns a bounded device error when installed metadata or package bytes
/// cannot be verified.
pub fn installed_using(root: &Path, key: &Ed25519PublicKey) -> Result<Vec<AppInfo>, DeviceError> {
    installed_with_key(root, key)
}

fn installed_with_key(root: &Path, key: &Ed25519PublicKey) -> Result<Vec<AppInfo>, DeviceError> {
    let mut entries = installed_manifests(root, key)?
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

/// Installs or updates one application from a verified channel cache.
///
/// # Errors
///
/// Returns a bounded device error when the identity, catalog, package,
/// signature, digest, or atomic transaction is invalid.
pub fn install(root: &Path, id: &str, channel: UpdateChannel) -> Result<(), DeviceError> {
    install_channel_with(root, id, channel, &public_key()?, |url, maximum| {
        kobo_net::fetch(url, maximum).map_err(network_error)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteInstallPlan {
    pub outcome: RemoteInstallOutcome,
    pub install: bool,
}

/// Plans a browser-requested install after refreshing its signed catalog.
///
/// # Errors
///
/// Returns a bounded device error when the catalog cannot be fetched,
/// verified, parsed, or cached.
pub fn prepare_remote_install(
    root: &Path,
    id: &str,
    channel: UpdateChannel,
) -> Result<RemoteInstallPlan, DeviceError> {
    let key = public_key()?;
    prepare_remote_install_channel_with(root, id, channel, &key, |url, maximum| {
        kobo_net::fetch(url, maximum).map_err(network_error)
    })
}

#[cfg(test)]
fn prepare_remote_install_with(
    root: &Path,
    id: &str,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<RemoteInstallPlan, DeviceError> {
    prepare_remote_install_channel_with(root, id, UpdateChannel::Stable, key, fetch)
}

fn prepare_remote_install_channel_with(
    root: &Path,
    id: &str,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
    mut fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<RemoteInstallPlan, DeviceError> {
    if !kobo_protocol::valid_app_id(id) || kobo_app_store::is_public_reserved_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    let source = catalog_source(channel);
    let json = fetch(source.url, CATALOG_LIMIT)?;
    let signature = fetch(source.signature_url, SIGNATURE_LIMIT)?;
    let catalog = verify_catalog(&json, &signature, key)?;
    write_channel_catalog_cache(root, channel, &json, &signature)?;
    let Some(entry) = catalog
        .entries()
        .iter()
        .find(|entry| entry.manifest().id() == id)
    else {
        return Ok(RemoteInstallPlan {
            outcome: RemoteInstallOutcome::Unavailable { id: id.to_owned() },
            install: false,
        });
    };
    if !kobo_app_store::cobalt_version_at_least(
        env!("CARGO_PKG_VERSION"),
        entry.manifest().minimum_cobalt_version(),
    ) {
        return Ok(RemoteInstallPlan {
            outcome: RemoteInstallOutcome::RequiresCobalt {
                id: id.to_owned(),
                minimum_cobalt_version: entry.manifest().minimum_cobalt_version().to_owned(),
            },
            install: false,
        });
    }

    let installed = installed_with_key(root, key)?;
    let current = installed.iter().find(|candidate| candidate.id == id);
    if current.and_then(|candidate| candidate.installed_version.as_deref())
        == Some(entry.manifest().version())
    {
        let included = managed_builtin(id).is_some()
            && builtin_is_installed(root, id)?
            && !safe_directory(&apps_root(root).join(id))?;
        return Ok(RemoteInstallPlan {
            outcome: if included {
                RemoteInstallOutcome::Included { id: id.to_owned() }
            } else {
                RemoteInstallOutcome::AlreadyInstalled { id: id.to_owned() }
            },
            install: false,
        });
    }
    if current.is_some()
        && !manifest_info(
            entry.manifest(),
            current.and_then(|candidate| candidate.installed_version.as_deref()),
        )?
        .has_update()
    {
        return Ok(RemoteInstallPlan {
            outcome: RemoteInstallOutcome::AlreadyInstalled { id: id.to_owned() },
            install: false,
        });
    }
    Ok(RemoteInstallPlan {
        outcome: if current.is_some() {
            RemoteInstallOutcome::Updated { id: id.to_owned() }
        } else {
            RemoteInstallOutcome::Installed { id: id.to_owned() }
        },
        install: true,
    })
}

/// Plans the same signed-catalog install used by browser links, with an
/// explicit transport and trust key for an isolated acceptance fixture.
///
/// # Errors
///
/// Returns a bounded device error when the fixture transport, catalog
/// verification, parsing, or cache activation fails.
pub fn prepare_remote_install_using(
    root: &Path,
    id: &str,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<RemoteInstallPlan, DeviceError> {
    prepare_remote_install_channel_with(root, id, channel, key, fetch)
}

#[must_use]
pub fn manages_builtin(id: &str) -> bool {
    managed_builtin(id).is_some()
}

#[must_use]
pub fn builtin_declared(id: &str) -> Option<kobo_policy::Declared> {
    let app = managed_builtin(id)?;
    kobo_policy::Declared::parse(app.capabilities.iter().copied()).ok()
}

/// Atomically removes one installed package while preserving application data.
///
/// # Errors
///
/// Returns a bounded device error for an invalid identity, missing package,
/// unsafe filesystem object, or failed transaction.
pub fn uninstall(root: &Path, id: &str) -> Result<(), DeviceError> {
    uninstall_using(root, id, &public_key()?)
}

/// Removes an application using an explicitly supplied trust key.
///
/// Application state is intentionally outside the installed package
/// directory and is not removed.
///
/// # Errors
///
/// Returns a bounded device error for an invalid identity, missing package,
/// failed verification, unsafe filesystem object, or failed transaction.
pub fn uninstall_using(root: &Path, id: &str, key: &Ed25519PublicKey) -> Result<(), DeviceError> {
    if !kobo_protocol::valid_app_id(id) || kobo_app_store::is_public_reserved_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    recover_interrupted_transaction(root, id, key)?;
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

/// Resolves an application binary after verifying its installed package.
///
/// # Errors
///
/// Returns a description when the identity is invalid, package is absent, or
/// installed metadata, signature, or binary bytes fail verification.
pub fn resolve(root: &Path, id: &str) -> Result<PathBuf, String> {
    let key = public_key().map_err(|error| format!("read app signing key: {error:?}"))?;
    resolve_using(root, id, &key)
}

/// Resolves a launchable application, preferring a verified Store package and
/// otherwise selecting a platform-owned built-in.
///
/// A committed uninstall tombstone always wins over the built-in fallback.
///
/// # Errors
///
/// Returns a description when the identity is invalid, a Store package fails
/// verification, the application was removed, or no executable exists.
pub fn resolve_launch(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !kobo_protocol::valid_app_id(id) || id == "kobod" {
        return Err(format!("{id:?} is not a valid application identity"));
    }
    let key = public_key().map_err(|error| format!("read app signing key: {error:?}"))?;
    if !kobo_app_store::is_public_reserved_app_id(id) {
        recover_interrupted_transaction(root, id, &key)
            .map_err(|error| format!("recover application {id}: {error:?}"))?;
        if is_uninstalled(root, id)
            .map_err(|error| format!("read application {id} state: {error:?}"))?
        {
            return Err(format!("no application named {id} is installed"));
        }
        let directory = apps_root(root).join(id);
        if safe_directory(&directory)
            .map_err(|error| format!("read application {id}: {error:?}"))?
        {
            let manifest = read_installed_manifest(&directory, id, &key)
                .map_err(|error| format!("installed application {id} is invalid: {error:?}"))?;
            return Ok(app_binary(root, manifest.id()));
        }
    }
    let builtin = builtin_binary(root, id);
    if safe_regular_file(&builtin)
        .map_err(|error| format!("read built-in application {id}: {error:?}"))?
    {
        return Ok(builtin);
    }
    Err(format!("no application named {id} is installed"))
}

/// Resolves and re-verifies an installed application with an explicit key.
///
/// # Errors
///
/// Returns a description when the identity is invalid, package is absent, or
/// installed metadata, signature, or binary bytes fail verification.
pub fn resolve_using(root: &Path, id: &str, key: &Ed25519PublicKey) -> Result<PathBuf, String> {
    if !kobo_protocol::valid_app_id(id) {
        return Err(format!("{id:?} is not a valid application identity"));
    }
    recover_interrupted_transaction(root, id, key)
        .map_err(|error| format!("recover application {id}: {error:?}"))?;
    if is_uninstalled(root, id)
        .map_err(|error| format!("read application {id} state: {error:?}"))?
    {
        return Err(format!("no application named {id} is installed"));
    }
    let directory = apps_root(root).join(id);
    if safe_directory(&directory).map_err(|error| format!("read application {id}: {error:?}"))? {
        let manifest = read_installed_manifest(&directory, id, key)
            .map_err(|error| format!("installed application {id} is invalid: {error:?}"))?;
        return Ok(app_binary(root, manifest.id()));
    }
    let builtin = builtin_binary(root, id);
    if managed_builtin(id).is_some() && builtin.is_file() {
        return Ok(builtin);
    }
    Err(format!("no application named {id} is installed"))
}

#[must_use]
pub fn declared(root: &Path, id: &str) -> Option<kobo_policy::Declared> {
    let key = public_key().ok()?;
    installed_manifest(root, id, &key)
        .ok()
        .map(|manifest| manifest.declared_capabilities().clone())
}

#[cfg(test)]
fn refresh_with(
    root: &Path,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<Vec<AppInfo>, DeviceError> {
    refresh_channel_with(root, UpdateChannel::Stable, key, fetch)
}

fn refresh_channel_with(
    root: &Path,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
    mut fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<Vec<AppInfo>, DeviceError> {
    let source = catalog_source(channel);
    let json = fetch(source.url, CATALOG_LIMIT)?;
    let signature = fetch(source.signature_url, SIGNATURE_LIMIT)?;
    let catalog = verify_catalog(&json, &signature, key)?;
    write_channel_catalog_cache(root, channel, &json, &signature)?;
    catalog_info(root, &catalog, key)
}

/// Fetches and atomically caches a signed catalog through an explicit
/// transport and trust key. This is the runtime path used by mock acceptance
/// runs; it does not weaken or replace device trust.
///
/// # Errors
///
/// Returns a bounded device error when fetching, verification, parsing, or
/// cache activation fails.
pub fn refresh_using(
    root: &Path,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<Vec<AppInfo>, DeviceError> {
    refresh_channel_with(root, channel, key, fetch)
}

#[cfg(test)]
fn install_with(
    root: &Path,
    id: &str,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<(), DeviceError> {
    install_channel_with(root, id, UpdateChannel::Stable, key, fetch)
}

fn install_channel_with(
    root: &Path,
    id: &str,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
    mut fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<(), DeviceError> {
    if !kobo_protocol::valid_app_id(id) || kobo_app_store::is_public_reserved_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    recover_interrupted_transaction(root, id, key)?;
    let catalog = read_channel_catalog(root, channel, key)?;
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
    if let Some(current) = installed_with_key(root, key)?
        .iter()
        .find(|candidate| candidate.id == id)
    {
        let candidate = manifest_info(entry.manifest(), current.installed_version.as_deref())?;
        if current.installed_version.as_deref() != Some(entry.manifest().version())
            && !candidate.has_update()
        {
            return Ok(());
        }
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

/// Installs through the runtime's verified package transaction using an
/// explicit fixture transport and trust key.
///
/// # Errors
///
/// Returns a bounded device error when the identity, catalog, package,
/// signature, digest, or atomic transaction is invalid.
pub fn install_using(
    root: &Path,
    id: &str,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<(), DeviceError> {
    install_channel_with(root, id, channel, key, fetch)
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

#[cfg(test)]
fn read_cached_catalog(root: &Path, key: &Ed25519PublicKey) -> Result<Catalog, DeviceError> {
    read_channel_catalog(root, UpdateChannel::Stable, key)
}

fn read_channel_catalog(
    root: &Path,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
) -> Result<Catalog, DeviceError> {
    recover_channel_catalog_cache(root, channel, key)?;
    let cache = catalog_cache(root, channel);
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
    key: &Ed25519PublicKey,
) -> Result<Vec<AppInfo>, DeviceError> {
    let installed = installed_with_key(root, key)?;
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
        minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
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
        minimum_cobalt_version: manifest.minimum_cobalt_version().to_owned(),
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
        match installed_manifest(root, &id, key) {
            Ok(manifest) => manifests.push(manifest),
            Err(DeviceError::NotFound) => {}
            Err(error) => return Err(error),
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
    let bytes = fs::read(directory.join("manifest.json")).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DeviceError::NotFound
        } else {
            DeviceError::Backend
        }
    })?;
    let manifest = Manifest::parse_public(&bytes).map_err(|_| DeviceError::Integrity)?;
    if manifest.id() != id || manifest.to_canonical_bytes() != bytes {
        return Err(DeviceError::Integrity);
    }
    let signature = fs::read_to_string(directory.join("manifest.json.sig"))
        .map_err(|error| installed_component_read_error(&error))?;
    let signature =
        DetachedSignature::from_hex(signature.trim()).map_err(|_| DeviceError::Integrity)?;
    verify(&bytes, &signature, key).map_err(|_| DeviceError::Integrity)?;
    let binary = fs::read(directory.join("bin").join(format!("kobo-{id}")))
        .map_err(|error| installed_component_read_error(&error))?;
    if binary.len() as u64 != manifest.binary_bytes()
        || kobo_net::sha256::hex_digest(&binary) != manifest.binary_sha256().as_str()
    {
        return Err(DeviceError::Integrity);
    }
    Ok(manifest)
}

fn installed_component_read_error(error: &std::io::Error) -> DeviceError {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData => DeviceError::Integrity,
        _ => DeviceError::Backend,
    }
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

#[cfg(test)]
fn write_catalog_cache(root: &Path, json: &[u8], signature: &[u8]) -> Result<(), DeviceError> {
    write_channel_catalog_cache(root, UpdateChannel::Stable, json, signature)
}

fn write_channel_catalog_cache(
    root: &Path,
    channel: UpdateChannel,
    json: &[u8],
    signature: &[u8],
) -> Result<(), DeviceError> {
    let store = cache_root(root);
    fs::create_dir_all(&store).map_err(|_| DeviceError::Backend)?;
    sync_directory(root)?;
    let source = catalog_source(channel);
    let current = catalog_cache(root, channel);
    let next = store.join(format!("{}.next", source.cache_name));
    let previous = store.join(format!("{}.prev", source.cache_name));
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

fn recover_channel_catalog_cache(
    root: &Path,
    channel: UpdateChannel,
    key: &Ed25519PublicKey,
) -> Result<(), DeviceError> {
    let source = catalog_source(channel);
    let current = catalog_cache(root, channel);
    if safe_directory(&current)? && read_catalog_directory(&current, key).is_ok() {
        return Ok(());
    }
    let previous = cache_root(root).join(format!("{}.prev", source.cache_name));
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

fn catalog_cache(root: &Path, channel: UpdateChannel) -> PathBuf {
    cache_root(root).join(catalog_source(channel).cache_name)
}

#[derive(Clone, Copy)]
struct CatalogSource {
    url: &'static str,
    signature_url: &'static str,
    cache_name: &'static str,
}

const fn catalog_source(channel: UpdateChannel) -> CatalogSource {
    match channel {
        UpdateChannel::Stable => CatalogSource {
            url: CATALOG_URL,
            signature_url: CATALOG_SIGNATURE_URL,
            cache_name: "catalog",
        },
        UpdateChannel::Beta => CatalogSource {
            url: BETA_CATALOG_URL,
            signature_url: BETA_CATALOG_SIGNATURE_URL,
            cache_name: "catalog-beta",
        },
    }
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
        "minus" => Glyph::Minus,
        "chess-white-king" => Glyph::ChessWhiteKing,
        "chess-white-queen" => Glyph::ChessWhiteQueen,
        "chess-white-rook" => Glyph::ChessWhiteRook,
        "chess-white-bishop" => Glyph::ChessWhiteBishop,
        "chess-white-knight" => Glyph::ChessWhiteKnight,
        "chess-white-pawn" => Glyph::ChessWhitePawn,
        "chess-black-king" => Glyph::ChessBlackKing,
        "chess-black-queen" => Glyph::ChessBlackQueen,
        "chess-black-rook" => Glyph::ChessBlackRook,
        "chess-black-bishop" => Glyph::ChessBlackBishop,
        "chess-black-knight" => Glyph::ChessBlackKnight,
        "chess-black-pawn" => Glyph::ChessBlackPawn,
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
        release_for_minimum(seed, id, version, env!("CARGO_PKG_VERSION"))
    }

    fn release_for_minimum(
        seed: &[u8; 32],
        id: &str,
        version: &str,
        minimum_cobalt_version: &str,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let binary = format!("{id} app binary").into_bytes();
        let manifest = Manifest::new_public(ManifestInput {
            id: id.to_owned(),
            display_name: format!("{id} application"),
            short_label: id.to_owned(),
            summary: format!("The {id} test application."),
            version: version.to_owned(),
            minimum_cobalt_version: minimum_cobalt_version.to_owned(),
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
        assert_eq!(
            resolve_launch(&root, "todo").expect("launch built-in"),
            builtin_binary(&root, "todo")
        );

        uninstall(&root, "todo").expect("uninstall built-in");
        assert!(installed(&root).expect("removed apps").is_empty());
        assert!(resolve(&root, "todo").is_err());
        assert!(resolve_launch(&root, "todo").is_err());

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
    fn an_installed_manifest_without_its_binary_fails_closed() {
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

        assert_eq!(
            resolve_using(&root, "word-count", &key).expect("resolve Store app"),
            app_binary(&root, "word-count")
        );
        assert!(!builtin_binary(&root, "word-count").exists());
        fs::remove_file(app_binary(&root, "word-count")).expect("remove installed binary");

        assert!(matches!(
            installed_with_key(&root, &key),
            Err(DeviceError::Integrity)
        ));
        assert!(resolve_using(&root, "word-count", &key)
            .expect_err("missing binary must not resolve")
            .contains("invalid"));
        assert_eq!(
            install_with(&root, "word-count", &key, |_, _| Ok(package.clone())),
            Err(DeviceError::Integrity)
        );

        uninstall_using(&root, "word-count", &key).expect("remove corrupt installation");
        install_with(&root, "word-count", &key, |_, _| Ok(package.clone()))
            .expect("clean reinstall");
        assert!(app_binary(&root, "word-count").is_file());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_install_plans_distinguish_new_updated_current_and_included_apps() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, package) = release(&seed);
        let plan = prepare_remote_install_with(&root, "word-count", &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("new plan");
        assert_eq!(
            plan,
            RemoteInstallPlan {
                outcome: RemoteInstallOutcome::Installed {
                    id: "word-count".to_owned()
                },
                install: true
            }
        );
        install_with(&root, "word-count", &key, |_, _| Ok(package.clone())).expect("install");
        let plan = prepare_remote_install_with(&root, "word-count", &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("current plan");
        assert_eq!(
            plan.outcome,
            RemoteInstallOutcome::AlreadyInstalled {
                id: "word-count".to_owned()
            }
        );
        assert!(!plan.install);

        let (updated_json, updated_signature, _) = release_for(&seed, "word-count", "1.1.0");
        let plan = prepare_remote_install_with(&root, "word-count", &key, |url, _| {
            if url == CATALOG_URL {
                Ok(updated_json.clone())
            } else {
                Ok(updated_signature.clone())
            }
        })
        .expect("update plan");
        assert_eq!(
            plan.outcome,
            RemoteInstallOutcome::Updated {
                id: "word-count".to_owned()
            }
        );
        assert!(plan.install);

        let included_root = root.with_extension("included");
        let _ignored = fs::remove_dir_all(&included_root);
        fs::create_dir_all(included_root.join("bin")).expect("built-in directory");
        fs::write(builtin_binary(&included_root, "todo"), b"built-in todo").expect("built-in Todo");
        let (included_json, included_signature, _) = release_for(&seed, "todo", "1.0.0");
        let plan = prepare_remote_install_with(&included_root, "todo", &key, |url, _| {
            if url == CATALOG_URL {
                Ok(included_json.clone())
            } else {
                Ok(included_signature.clone())
            }
        })
        .expect("included plan");
        assert_eq!(
            plan.outcome,
            RemoteInstallOutcome::Included {
                id: "todo".to_owned()
            }
        );
        assert!(!plan.install);
        let _ignored = fs::remove_dir_all(root);
        let _ignored = fs::remove_dir_all(included_root);
    }

    #[test]
    fn an_installed_app_absent_from_the_signed_catalog_is_unavailable() {
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

        let (other_json, other_signature, _) = release_for(&seed, "notes", "1.0.0");
        let plan = prepare_remote_install_with(&root, "word-count", &key, |url, _| {
            if url == CATALOG_URL {
                Ok(other_json.clone())
            } else {
                Ok(other_signature.clone())
            }
        })
        .expect("unavailable plan");
        assert_eq!(
            plan.outcome,
            RemoteInstallOutcome::Unavailable {
                id: "word-count".to_owned()
            }
        );
        assert!(!plan.install);
        let installed = installed_with_key(&root, &key).expect("installed app remains");
        assert!(installed.iter().any(|app| app.id == "word-count"));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_remote_app_requiring_newer_cobalt_reports_the_required_version() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (json, signature, _) = release_for_minimum(&seed, "word-count", "1.0.0", "999.0.0");
        let plan = prepare_remote_install_with(&root, "word-count", &key, |url, _| {
            if url == CATALOG_URL {
                Ok(json.clone())
            } else {
                Ok(signature.clone())
            }
        })
        .expect("compatibility plan");
        assert_eq!(
            plan.outcome,
            RemoteInstallOutcome::RequiresCobalt {
                id: "word-count".to_owned(),
                minimum_cobalt_version: "999.0.0".to_owned(),
            }
        );
        assert!(!plan.install);
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
    fn stable_and_beta_catalogs_have_separate_urls_and_verified_caches() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (stable_json, stable_signature, _) = release_for(&seed, "word-count", "1.0.0");
        let (beta_json, beta_signature, _) = release_for(&seed, "word-count", "2.0.0");

        refresh_channel_with(&root, UpdateChannel::Stable, &key, |url, _| match url {
            CATALOG_URL => Ok(stable_json.clone()),
            CATALOG_SIGNATURE_URL => Ok(stable_signature.clone()),
            _ => Err(DeviceError::NotFound),
        })
        .expect("stable refresh");
        refresh_channel_with(&root, UpdateChannel::Beta, &key, |url, _| match url {
            BETA_CATALOG_URL => Ok(beta_json.clone()),
            BETA_CATALOG_SIGNATURE_URL => Ok(beta_signature.clone()),
            _ => Err(DeviceError::NotFound),
        })
        .expect("beta refresh");

        assert_eq!(
            read_channel_catalog(&root, UpdateChannel::Stable, &key)
                .expect("stable cache")
                .entries()[0]
                .manifest()
                .version(),
            "1.0.0"
        );
        assert_eq!(
            read_channel_catalog(&root, UpdateChannel::Beta, &key)
                .expect("beta cache")
                .entries()[0]
                .manifest()
                .version(),
            "2.0.0"
        );
        assert!(catalog_cache(&root, UpdateChannel::Stable).is_dir());
        assert!(catalog_cache(&root, UpdateChannel::Beta).is_dir());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn opting_out_of_beta_never_downgrades_an_installed_app() {
        let root = root();
        let seed = [9_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let (stable_json, stable_signature, stable_package) =
            release_for(&seed, "word-count", "1.0.0");
        let (beta_json, beta_signature, beta_package) = release_for(&seed, "word-count", "2.0.0");
        refresh_channel_with(&root, UpdateChannel::Beta, &key, |url, _| {
            if url == BETA_CATALOG_URL {
                Ok(beta_json.clone())
            } else {
                Ok(beta_signature.clone())
            }
        })
        .expect("beta refresh");
        install_channel_with(&root, "word-count", UpdateChannel::Beta, &key, |_, _| {
            Ok(beta_package.clone())
        })
        .expect("beta install");
        refresh_channel_with(&root, UpdateChannel::Stable, &key, |url, _| {
            if url == CATALOG_URL {
                Ok(stable_json.clone())
            } else {
                Ok(stable_signature.clone())
            }
        })
        .expect("stable refresh");

        let stable_listing = catalog_info(
            &root,
            &read_channel_catalog(&root, UpdateChannel::Stable, &key).expect("stable cache"),
            &key,
        )
        .expect("stable listing");
        let entry = stable_listing
            .iter()
            .find(|entry| entry.id == "word-count")
            .expect("word-count");
        assert_eq!(entry.installed_version.as_deref(), Some("2.0.0"));
        assert!(!entry.has_update());

        install_channel_with(&root, "word-count", UpdateChannel::Stable, &key, |_, _| {
            Ok(stable_package.clone())
        })
        .expect("stable install is a no-op");
        assert_eq!(
            installed_manifest(&root, "word-count", &key)
                .expect("installed beta remains")
                .version(),
            "2.0.0"
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
        let clean_reinstall = || {
            uninstall_using(&root, "word-count", &key).expect("remove corrupt installation");
            install();
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

        clean_reinstall();
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

        clean_reinstall();
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
        fs::rename(
            catalog_cache(&root, UpdateChannel::Stable),
            store.join("catalog.prev"),
        )
        .expect("retire cache");
        fs::create_dir(store.join("catalog.next")).expect("stage cache");
        fs::write(store.join("catalog.next/catalog.json"), b"incomplete").expect("partial cache");

        assert_eq!(
            read_cached_catalog(&root, &key)
                .expect("recover cache")
                .entries()
                .len(),
            1
        );
        assert!(catalog_cache(&root, UpdateChannel::Stable)
            .join("catalog.json.sig")
            .is_file());
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
