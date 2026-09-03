//! Background updates the runtime performs on its own.
//!
//! Two switches, both on until the owner turns one off: the platform keeps
//! itself current, and installed applications keep themselves current. The
//! switches live in a small file under the runtime's state directory rather
//! than in memory, because the choice belongs to the owner and has to outlive
//! every session and every update.
//!
//! Everything here is deliberately quiet. Checking is done from a background
//! thread; applying is done by the panel loop when nobody has touched the
//! screen for a while. A failure leaves a line in the trace and nothing on
//! the panel: a reader who wants to watch an update happen has the settings
//! screen, which is exactly what it is for.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use kobo_protocol::{DeviceError, UpdateChannel};

/// Where releases are published. The same address the settings screen asks,
/// so the two paths cannot disagree about what "newest" means.
const STABLE_RELEASES: &str = "https://api.github.com/repos/BandarLabs/Cobalt/releases/latest";
const BETA_RELEASES: &str = "https://api.github.com/repos/BandarLabs/Cobalt/releases?per_page=100";

/// The most a release description is allowed to be. The real reply is a few
/// kilobytes; a reply a thousand times that size is not a release description.
const RELEASE_LIMIT: u32 = 1024 * 1024;

/// The most a digest listing is allowed to be: a few lines of hex and names.
const DIGEST_LIMIT: u32 = 16 * 1024;

/// The file holding the owner's two choices, under `root/state`.
const STATE_FILE: &str = "auto-update";

/// The owner's standing choices about background updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Preferences {
    /// Whether the platform replaces itself when a newer release is published.
    pub cobalt: bool,
    /// Whether installed applications are replaced when the catalog moves on.
    pub apps: bool,
    /// Which platform and application release streams are consulted.
    pub channel: UpdateChannel,
}

impl Default for Preferences {
    /// Both on. A reader who never opens the settings screen still gets
    /// fixes, which is the point of publishing them.
    fn default() -> Self {
        Self {
            cobalt: true,
            apps: true,
            channel: UpdateChannel::Stable,
        }
    }
}

/// Everything the checker found that is newer than what is installed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    /// The channel this plan was resolved from.
    pub channel: UpdateChannel,
    /// Catalog applications whose installed version is no longer current.
    pub apps: Vec<String>,
    /// A newer platform release, verified digest and all, or nothing.
    pub platform: Option<PlatformUpdate>,
}

impl Plan {
    /// Whether there is anything to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty() && self.platform.is_none()
    }
}

/// One installable platform release with the digest that vouches for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformUpdate {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// Reads the owner's choices. A missing or unreadable file is the default,
/// not an error: the file only exists once somebody has turned something off
/// or on again, and before that the answer is "both on".
#[must_use]
pub fn preferences(root: &Path) -> Preferences {
    fs::read_to_string(state_file(root))
        .ok()
        .map_or_else(Preferences::default, |text| parse(&text))
}

/// Records the owner's choices so they survive restarts and updates.
///
/// # Errors
///
/// [`DeviceError::Backend`] when the book partition refuses the write.
pub fn set_preferences(root: &Path, chosen: Preferences) -> Result<(), DeviceError> {
    let path = state_file(root);
    let parent = path.parent().ok_or(DeviceError::Backend)?;
    fs::create_dir_all(parent).map_err(|_| DeviceError::Backend)?;
    // Written beside and renamed over, so a power cut mid-write leaves the
    // old choices rather than half a file.
    let next = path.with_extension("next");
    let mut file = fs::File::create(&next).map_err(|_| DeviceError::Backend)?;
    file.write_all(render(chosen).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|_| DeviceError::Backend)?;
    fs::rename(&next, &path).map_err(|_| DeviceError::Backend)?;
    // The rename is a directory entry, so the directory is synced too, or a
    // power cut could forget a choice the file itself already kept.
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DeviceError::Backend)
}

/// Asks the catalog and the release feed what is newer than what is running.
///
/// Only the sections the owner left on are consulted. Network trouble yields
/// an empty section rather than an error, because the checker will simply ask
/// again later and there is nobody at the panel to tell.
#[must_use]
pub fn plan(root: &Path, chosen: Preferences, installed: &str) -> Plan {
    plan_with(
        chosen,
        || stale_apps(root, chosen.channel),
        || platform_update(installed, chosen.channel),
    )
}

fn plan_with(
    chosen: Preferences,
    apps: impl FnOnce() -> Vec<String>,
    platform: impl FnOnce() -> Option<PlatformUpdate>,
) -> Plan {
    let apps = if chosen.apps { apps() } else { Vec::new() };
    let platform = if chosen.cobalt { platform() } else { None };
    Plan {
        channel: chosen.channel,
        apps,
        platform,
    }
}

fn state_file(root: &Path) -> PathBuf {
    root.join("state").join(STATE_FILE)
}

/// One `name=on|off` line per switch. Anything unrecognised is ignored and
/// anything missing is on, so a file written by a newer build still reads.
fn parse(text: &str) -> Preferences {
    let mut chosen = Preferences::default();
    for line in text.lines() {
        match line.trim() {
            "cobalt=off" => chosen.cobalt = false,
            "cobalt=on" => chosen.cobalt = true,
            "apps=off" => chosen.apps = false,
            "apps=on" => chosen.apps = true,
            "channel=stable" => chosen.channel = UpdateChannel::Stable,
            "channel=beta" => chosen.channel = UpdateChannel::Beta,
            _ => {}
        }
    }
    chosen
}

fn render(chosen: Preferences) -> String {
    format!(
        "cobalt={}\napps={}\nchannel={}\n",
        if chosen.cobalt { "on" } else { "off" },
        if chosen.apps { "on" } else { "off" },
        match chosen.channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        }
    )
}

/// Catalog applications that are installed at one version while the catalog
/// publishes another. The refresh both fetches and verifies the catalog, so
/// an identity that cannot be vouched for never reaches the list.
fn stale_apps(root: &Path, channel: UpdateChannel) -> Vec<String> {
    crate::app_store::refresh(root, channel).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .into_iter()
                .filter(kobo_protocol::AppInfo::has_update)
                .map(|entry| entry.id)
                .collect()
        },
    )
}

/// A strictly newer published release, with its digest already fetched, or
/// nothing. All-or-nothing on purpose: a release that cannot be verified is
/// not an update, it is a download.
fn platform_update(installed: &str, channel: UpdateChannel) -> Option<PlatformUpdate> {
    let endpoint = match channel {
        UpdateChannel::Stable => STABLE_RELEASES,
        UpdateChannel::Beta => BETA_RELEASES,
    };
    let body = kobo_net::fetch(endpoint, RELEASE_LIMIT).ok()?;
    let body = String::from_utf8(body).ok()?;
    let (version, archive, digest_url) = match channel {
        UpdateChannel::Stable => release_from(&body),
        UpdateChannel::Beta => beta_release_from(&body),
    }?;
    if !newer(&version, installed) {
        return None;
    }
    let listing = kobo_net::fetch(&digest_url, DIGEST_LIMIT).ok()?;
    let listing = String::from_utf8(listing).ok()?;
    let sha256 = digest_from(&listing, &archive_file(&version))?;
    Some(PlatformUpdate {
        version,
        url: archive,
        sha256,
    })
}

/// Reads the GitHub "latest release" reply down to the version, the archive
/// URL and the digest URL this device needs.
fn release_from(body: &str) -> Option<(String, String, String)> {
    let value = kobo_json::parse(body).ok()?;
    release_value(&value, "v", false)
}

/// Selects the highest valid beta prerelease, independent of GitHub's reply
/// order. Drafts, stable releases and malformed beta entries are skipped.
fn beta_release_from(body: &str) -> Option<(String, String, String)> {
    let value = kobo_json::parse(body).ok()?;
    value
        .as_array()?
        .iter()
        .filter_map(|release| release_value(release, "beta-v", true))
        .max_by_key(|(version, _, _)| numbers(version))
}

fn release_value(
    value: &kobo_json::Value,
    tag_prefix: &str,
    require_prerelease: bool,
) -> Option<(String, String, String)> {
    if value
        .get("draft")
        .and_then(kobo_json::Value::as_bool)
        .unwrap_or(false)
        || value
            .get("prerelease")
            .and_then(kobo_json::Value::as_bool)
            .unwrap_or(false)
            != require_prerelease
    {
        return None;
    }
    let tag = value.get("tag_name").and_then(kobo_json::Value::as_str)?;
    let version = tag.strip_prefix(tag_prefix)?.to_owned();
    numbers(&version)?;
    let assets = value.get("assets").and_then(kobo_json::Value::as_array)?;
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|asset| asset.get("name").and_then(kobo_json::Value::as_str) == Some(name))
            .and_then(|asset| asset.get("browser_download_url"))
            .and_then(kobo_json::Value::as_str)
            .filter(|url| valid_release_url(url))
            .map(str::to_owned)
    };
    let archive = url_of(&archive_file(&version))?;
    let digest = url_of(&format!("cobalt-{version}.sha256"))?;
    Some((version, archive, digest))
}

fn valid_release_url(url: &str) -> bool {
    url.starts_with("https://") && url.len() <= kobo_protocol::MAX_URL_LEN
}

fn archive_file(version: &str) -> String {
    format!("cobalt-{version}-KoboRoot.tgz")
}

/// Strictly newer, so the same version and anything unparseable both answer
/// no and nothing is replaced on a guess.
fn newer(latest: &str, installed: &str) -> bool {
    match (numbers(latest), numbers(installed)) {
        (Some(latest), Some(installed)) => latest > installed,
        _ => false,
    }
}

fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => Some((major, minor, patch)),
        _ => None,
    }
}

/// Finds the digest vouching for `asset` in a `sha256sum` style listing:
/// sixty-four hex characters, whitespace, a file name per line.
fn digest_from(listing: &str, asset: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (digest, name) = line.split_once(char::is_whitespace)?;
        let named = name.trim_start().trim_start_matches('*') == asset;
        let plausible = digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        (named && plausible).then(|| digest.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        beta_release_from, digest_from, newer, parse, plan_with, preferences, release_from, render,
        set_preferences, PlatformUpdate, Preferences,
    };
    use kobo_protocol::UpdateChannel;

    #[test]
    fn a_reader_who_chose_nothing_gets_both_updates() {
        let chosen = Preferences::default();
        assert!(chosen.cobalt);
        assert!(chosen.apps);
        assert_eq!(chosen.channel, UpdateChannel::Stable);
    }

    #[test]
    fn choices_survive_being_written_and_read_back() {
        for cobalt in [false, true] {
            for apps in [false, true] {
                for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
                    let chosen = Preferences {
                        cobalt,
                        apps,
                        channel,
                    };
                    assert_eq!(parse(&render(chosen)), chosen);
                }
            }
        }
    }

    #[test]
    fn a_file_a_newer_build_wrote_still_reads() {
        // Unknown lines are ignored and missing lines mean on, so a file with
        // switches this build has never heard of is not a reason to fail.
        let chosen = parse("cobalt=off\ndictionaries=off\n");
        assert!(!chosen.cobalt);
        assert!(chosen.apps);
        assert_eq!(chosen.channel, UpdateChannel::Stable);
    }

    #[test]
    fn legacy_and_malformed_channel_values_remain_stable() {
        assert_eq!(
            parse("cobalt=off\napps=off\n").channel,
            UpdateChannel::Stable
        );
        assert_eq!(
            parse("channel=nightly\ncobalt=off\n").channel,
            UpdateChannel::Stable
        );
        assert_eq!(
            parse("channel=beta\nfuture=value\n").channel,
            UpdateChannel::Beta
        );
    }

    #[test]
    fn a_missing_file_is_the_default_and_a_write_makes_it_stick() {
        let root = std::env::temp_dir().join(format!("cobalt-auto-update-{}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&root);
        assert_eq!(preferences(&root), Preferences::default());
        let chosen = Preferences {
            cobalt: false,
            apps: true,
            channel: UpdateChannel::Beta,
        };
        set_preferences(&root, chosen).expect("write choices");
        assert_eq!(preferences(&root), chosen);
        let _ignored = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_strictly_newer_triple_counts() {
        assert!(newer("0.4.0", "0.3.0"));
        assert!(!newer("0.3.0", "0.3.0"));
        assert!(!newer("0.2.9", "0.3.0"));
        assert!(!newer("0.4", "0.3.0"));
        assert!(!newer("0.4.0", "unknown"));
    }

    #[test]
    fn a_release_is_read_down_to_its_three_facts() {
        let body = r#"{
            "tag_name": "v0.4.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": "cobalt-0.4.0-KoboRoot.tgz",
                 "browser_download_url": "https://example.test/cobalt-0.4.0-KoboRoot.tgz"},
                {"name": "cobalt-0.4.0.sha256",
                 "browser_download_url": "https://example.test/cobalt-0.4.0.sha256"}
            ]
        }"#;
        let (version, archive, digest) = release_from(body).expect("release");
        assert_eq!(version, "0.4.0");
        assert_eq!(archive, "https://example.test/cobalt-0.4.0-KoboRoot.tgz");
        assert_eq!(digest, "https://example.test/cobalt-0.4.0.sha256");
    }

    #[test]
    fn a_release_missing_either_asset_offers_nothing() {
        assert!(release_from(r#"{"tag_name": "v0.4.0", "assets": []}"#).is_none());
        assert!(release_from(r#"{"assets": []}"#).is_none());
        assert!(
            release_from(r#"{"tag_name":"beta-v0.4.0","prerelease":true,"assets":[]}"#).is_none()
        );
    }

    #[test]
    fn beta_feed_selects_the_highest_valid_prerelease() {
        let body = r#"[
          {"tag_name":"beta-v0.4.0","draft":false,"prerelease":true,"assets":[
            {"name":"cobalt-0.4.0-KoboRoot.tgz","browser_download_url":"https://example.test/0.4.0.tgz"},
            {"name":"cobalt-0.4.0.sha256","browser_download_url":"https://example.test/0.4.0.sha256"}]},
          {"tag_name":"beta-v0.6.0","draft":true,"prerelease":true,"assets":[
            {"name":"cobalt-0.6.0-KoboRoot.tgz","browser_download_url":"https://example.test/0.6.0.tgz"},
            {"name":"cobalt-0.6.0.sha256","browser_download_url":"https://example.test/0.6.0.sha256"}]},
          {"tag_name":"v0.7.0","draft":false,"prerelease":false,"assets":[]},
          {"tag_name":"beta-v0.8.0","draft":false,"prerelease":true,"assets":[
            {"name":"KoboRoot.tgz","browser_download_url":"https://example.test/wrong.tgz"},
            {"name":"cobalt-0.8.0.sha256","browser_download_url":"https://example.test/0.8.0.sha256"}]},
          {"tag_name":"beta-v0.5.0","draft":false,"prerelease":true,"assets":[
            {"name":"cobalt-0.5.0-KoboRoot.tgz","browser_download_url":"https://example.test/0.5.0.tgz"},
            {"name":"cobalt-0.5.0.sha256","browser_download_url":"https://example.test/0.5.0.sha256"}]}
        ]"#;
        let (version, archive, digest) = beta_release_from(body).expect("beta release");
        assert_eq!(version, "0.5.0");
        assert_eq!(archive, "https://example.test/0.5.0.tgz");
        assert_eq!(digest, "https://example.test/0.5.0.sha256");
    }

    #[test]
    fn malformed_and_non_beta_feeds_offer_nothing() {
        assert!(beta_release_from("not json").is_none());
        assert!(beta_release_from(r#"{"tag_name":"beta-v0.4.0"}"#).is_none());
        assert!(beta_release_from(
            r#"[{"tag_name":"beta-vnext","draft":false,"prerelease":true,"assets":[]}]"#
        )
        .is_none());
        assert!(beta_release_from(
            r#"[{"tag_name":"beta-v0.4.0","draft":false,"prerelease":false,"assets":[]}]"#
        )
        .is_none());
    }

    #[test]
    fn the_digest_listing_is_matched_by_name_and_shape() {
        let listing = format!(
            "{}  cobalt-0.4.0-KoboRoot.tgz\n{}  something-else.tgz\n",
            "a".repeat(64),
            "b".repeat(64)
        );
        assert_eq!(
            digest_from(&listing, "cobalt-0.4.0-KoboRoot.tgz"),
            Some("a".repeat(64))
        );
        assert_eq!(digest_from(&listing, "cobalt-0.5.0-KoboRoot.tgz"), None);
        assert_eq!(
            digest_from(
                "tooshort  cobalt-0.4.0-KoboRoot.tgz",
                "cobalt-0.4.0-KoboRoot.tgz"
            ),
            None
        );
    }

    #[test]
    fn update_plan_checks_only_enabled_sections_in_every_channel() {
        for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
            for cobalt in [false, true] {
                for apps in [false, true] {
                    let mut app_checks = 0;
                    let mut platform_checks = 0;
                    let chosen = Preferences {
                        cobalt,
                        apps,
                        channel,
                    };
                    let plan = plan_with(
                        chosen,
                        || {
                            app_checks += 1;
                            vec!["todo".to_owned()]
                        },
                        || {
                            platform_checks += 1;
                            Some(PlatformUpdate {
                                version: "9.9.9".to_owned(),
                                url: "https://example.test/cobalt.tgz".to_owned(),
                                sha256: "a".repeat(64),
                            })
                        },
                    );
                    assert_eq!(app_checks, usize::from(apps));
                    assert_eq!(platform_checks, usize::from(cobalt));
                    assert_eq!(plan.channel, channel);
                    assert_eq!(plan.apps.is_empty(), !apps);
                    assert_eq!(plan.platform.is_none(), !cobalt);
                }
            }
        }
    }
}
