//! Reusable Beta Store acceptance and evidence collection.

use kobo_app_store::{
    build_bundle, derive_public_key, parse_public_bundle, sign, verify, Catalog, CatalogEntry,
    CatalogEntryInput, DetachedSignature, Ed25519PublicKey, Manifest, ManifestInput,
    PUBLIC_RELEASE_KEY_HEX,
};
use kobo_protocol::{DeviceError, RemoteInstallOutcome, UpdateChannel};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const USAGE: &str = "usage:\n  \
  kobo beta-store-smoke --app ID --fixture DIR --out DIR [--marketing-route PATH] [--dry-run]\n  \
  kobo beta-store-smoke --app ID --beta-catalog URL --device IP --out DIR \\\n+    --expected-profile PROFILE --expected-cobalt VERSION --expected-firmware VERSION \\\n+    --marketing-route PATH --confirm PROFILE/Cobalt-VERSION/FIRMWARE";
const FIXTURE_SEED: &str = "fixture-seed.hex";
const FIXTURE_MARKETING_ROUTE: &str = "marketing-route.txt";
const DEVICE_UNLOCK: &str = "OWNER_ATTENDED_BETA_STORE_ACCEPTANCE";
const PHYSICAL_LAUNCH_SECONDS: u64 = 45;
const MARKETING_FPS: u32 = 4;
const MARKETING_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    app: String,
    out: PathBuf,
    fixture: Option<PathBuf>,
    beta_catalog: Option<String>,
    device: Option<String>,
    expected_profile: Option<String>,
    expected_cobalt: Option<String>,
    expected_firmware: Option<String>,
    confirmation: Option<String>,
    marketing_route: Option<PathBuf>,
    dry_run: bool,
}

#[derive(Clone)]
struct Release {
    version: String,
    catalog: Vec<u8>,
    catalog_signature: Vec<u8>,
    package: Vec<u8>,
    package_url: String,
    binary_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppSnapshot {
    installed: bool,
    version: Option<String>,
    binary_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Preservation {
    unrelated: String,
    state: String,
    target_state: String,
}

#[derive(Clone, Debug)]
struct Event {
    name: String,
    result: String,
    detail: String,
}

struct Evidence {
    mode: &'static str,
    app: String,
    catalog_url: String,
    catalog_sha256: String,
    catalog_signature: String,
    public_key_fingerprint: String,
    package_sha256: String,
    package_bytes: usize,
    before: AppSnapshot,
    after: AppSnapshot,
    preservation_before: Preservation,
    preservation_after: Preservation,
    marketing: MarketingEvidence,
    events: Vec<Event>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarketingRoute {
    title: String,
    interaction: String,
    result: String,
    x: u32,
    y: u32,
    title_ms: u64,
    result_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarketingArtifact {
    kind: String,
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarketingEvidence {
    complete: bool,
    reason: Option<String>,
    title: String,
    interaction: String,
    result: String,
    width: u32,
    height: u32,
    frame_count: usize,
    duration_ms: u64,
    fps: u32,
    reproducible_command: String,
    artifacts: Vec<MarketingArtifact>,
}

struct RunLock {
    path: PathBuf,
}

impl RunLock {
    fn acquire(out: &Path) -> Result<Self, String> {
        fs::create_dir_all(out)
            .map_err(|error| format!("create evidence directory {}: {error}", out.display()))?;
        let path = out.join(".beta-store-smoke.lock");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "another acceptance command owns {}: {error}",
                    path.display()
                )
            })?;
        writeln!(file, "pid={}", std::process::id())
            .map_err(|error| format!("write command lock: {error}"))?;
        Ok(Self { path })
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ignored = fs::remove_file(&self.path);
    }
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    let options = parse(arguments)?;
    if options.out.join("report.json").exists() {
        return Err(format!(
            "{} already contains acceptance evidence; choose a new directory",
            options.out.display()
        ));
    }
    let _lock = RunLock::acquire(&options.out)?;
    match (&options.fixture, &options.device) {
        (Some(fixture), None) => run_mock(&options, fixture),
        (None, Some(device)) => run_physical(&options, device),
        _ => Err(USAGE.to_owned()),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the fail-closed mode and attended confirmation grammar are parsed together"
)]
fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut app = None;
    let mut out = None;
    let mut fixture = None;
    let mut beta_catalog = None;
    let mut device = None;
    let mut expected_profile = None;
    let mut expected_cobalt = None;
    let mut expected_firmware = None;
    let mut confirmation = None;
    let mut marketing_route = None;
    let mut dry_run = false;
    let mut index = 0;
    while index < arguments.len() {
        let slot = match arguments[index].as_str() {
            "--app" => &mut app,
            "--out" => &mut out,
            "--fixture" => &mut fixture,
            "--beta-catalog" => &mut beta_catalog,
            "--device" | "-s" => &mut device,
            "--expected-profile" => &mut expected_profile,
            "--expected-cobalt" => &mut expected_cobalt,
            "--expected-firmware" => &mut expected_firmware,
            "--confirm" => &mut confirmation,
            "--marketing-route" => &mut marketing_route,
            "--dry-run" => {
                dry_run = true;
                index += 1;
                continue;
            }
            _ => return Err(USAGE.to_owned()),
        };
        let value = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{} was supplied more than once", arguments[index]));
        }
        index += 2;
    }
    let app = app.ok_or_else(|| USAGE.to_owned())?;
    if !kobo_protocol::valid_app_id(&app) || kobo_app_store::is_public_reserved_app_id(&app) {
        return Err("app ID is not a valid public Store identity".to_owned());
    }
    let out = out.map(PathBuf::from).ok_or_else(|| USAGE.to_owned())?;
    let fixture = fixture.map(PathBuf::from);
    match (&fixture, &beta_catalog, &device) {
        (Some(_), None, None) => {
            if expected_profile.is_some()
                || expected_cobalt.is_some()
                || expected_firmware.is_some()
                || confirmation.is_some()
            {
                return Err(
                    "device confirmation options are not accepted in fixture mode".to_owned(),
                );
            }
        }
        (None, Some(url), Some(host)) => {
            if url != kobod::app_store::BETA_CATALOG_URL {
                return Err(format!(
                    "refusing non-Beta catalog {url:?}; the only attended source is {}",
                    kobod::app_store::BETA_CATALOG_URL
                ));
            }
            if !crate::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            if dry_run {
                return Err(
                    "--dry-run is fixture-only; attended mode performs real checks".to_owned(),
                );
            }
            if marketing_route.is_none() {
                return Err("attended mode requires --marketing-route PATH".to_owned());
            }
            let expected = match (
                expected_profile.as_deref(),
                expected_cobalt.as_deref(),
                expected_firmware.as_deref(),
            ) {
                (Some(profile), Some(cobalt), Some(firmware)) => {
                    format!("{profile}/Cobalt-{cobalt}/{firmware}")
                }
                _ => return Err(USAGE.to_owned()),
            };
            if confirmation.as_deref() != Some(&expected) {
                return Err(format!("confirmation must be exactly {expected:?}"));
            }
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(Options {
        app,
        out,
        fixture,
        beta_catalog,
        device,
        expected_profile,
        expected_cobalt,
        expected_firmware,
        confirmation,
        marketing_route: marketing_route.map(PathBuf::from),
        dry_run,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "the ordered acceptance matrix is easier to audit as one transaction"
)]
fn run_mock(options: &Options, fixture: &Path) -> Result<(), String> {
    let seed = read_seed(&fixture.join(FIXTURE_SEED))?;
    let route_path = options
        .marketing_route
        .clone()
        .unwrap_or_else(|| fixture.join(FIXTURE_MARKETING_ROUTE));
    let marketing_route = read_marketing_route(&route_path)?;
    let key = derive_public_key(&seed).map_err(|error| format!("derive fixture key: {error}"))?;
    let baseline = release(&options.app, "1.0.0", true, &seed)?;
    let target = release(&options.app, "1.1.0", true, &seed)?;
    let downgrade = release(&options.app, "0.9.0", true, &seed)?;
    let launch_failure = release(&options.app, "1.1.0", false, &seed)?;
    verify_release(&target, &key)?;

    fs::write(options.out.join("catalog.json"), &target.catalog)
        .map_err(|error| format!("write catalog evidence: {error}"))?;
    fs::write(
        options.out.join("catalog.json.sig"),
        &target.catalog_signature,
    )
    .map_err(|error| format!("write catalog signature evidence: {error}"))?;
    fs::write(
        options.out.join(format!("{}.cobalt-app", options.app)),
        &target.package,
    )
    .map_err(|error| format!("write package evidence: {error}"))?;

    let empty = AppSnapshot {
        installed: false,
        version: None,
        binary_sha256: None,
    };
    let empty_preservation = Preservation {
        unrelated: crate::sha256::hex_digest(b"dry-run"),
        state: crate::sha256::hex_digest(b"dry-run"),
        target_state: crate::sha256::hex_digest(b"dry-run"),
    };
    let mut evidence = Evidence {
        mode: if options.dry_run { "dry-run" } else { "mock" },
        app: options.app.clone(),
        catalog_url: "local-fixture://beta/catalog.json".to_owned(),
        catalog_sha256: crate::sha256::hex_digest(&target.catalog),
        catalog_signature: String::from_utf8_lossy(&target.catalog_signature)
            .trim()
            .to_owned(),
        public_key_fingerprint: crate::sha256::hex_digest(key.as_bytes()),
        package_sha256: crate::sha256::hex_digest(&target.package),
        package_bytes: target.package.len(),
        before: empty.clone(),
        after: empty,
        preservation_before: empty_preservation.clone(),
        preservation_after: empty_preservation,
        marketing: MarketingEvidence::pending(&marketing_route),
        events: vec![Event {
            name: "fixture-verification".to_owned(),
            result: "passed".to_owned(),
            detail: "canonical catalog, detached signature, package digest, manifest signature and binary digest verified"
                .to_owned(),
        }],
    };
    if options.dry_run {
        evidence.marketing =
            create_mock_marketing(&options.out, &marketing_route, discover_encoder_tools())?;
        evidence.events.push(Event {
            name: "marketing-capture".to_owned(),
            result: if evidence.marketing.complete {
                "passed"
            } else {
                "incomplete"
            }
            .to_owned(),
            detail: evidence
                .marketing
                .reason
                .clone()
                .unwrap_or_else(|| "validated mock marketing artifacts were produced".to_owned()),
        });
        evidence.events.push(Event {
            name: "dry-run".to_owned(),
            result: "passed".to_owned(),
            detail: "no simulated device state was created or changed".to_owned(),
        });
        write_report(&options.out, &evidence)?;
        if !evidence.marketing.complete {
            return Err("marketing capture is incomplete; see report.json".to_owned());
        }
        println!("beta Store dry-run evidence: {}", options.out.display());
        return Ok(());
    }

    let root = options.out.join("mock-device");
    seed_owner_state(&root, &options.app)?;
    evidence.before = app_snapshot(&root, &options.app, &key)?;
    evidence.preservation_before = preservation(&root, &options.app)?;

    expect_install(&root, &options.app, &target, &key, "clean install")?;
    event(
        &mut evidence,
        "clean-install",
        "target package installed through the verified Beta cache",
    );
    launch(&root, &options.app, &key, true)?;
    mock_shot(&options.out.join("01-clean-install.png"), 36)?;
    event(
        &mut evidence,
        "launch-success",
        "installed executable launched and returned success",
    );

    let rerun = plan(&root, &options.app, &target, &key)?;
    ensure(
        !rerun.install && matches!(rerun.outcome, RemoteInstallOutcome::AlreadyInstalled { .. }),
        "already-installed rerun did not close as a no-op",
    )?;
    event(
        &mut evidence,
        "already-installed-rerun",
        "reported AlreadyInstalled without replacing bytes",
    );

    kobod::app_store::uninstall_using(&root, &options.app, &key)
        .map_err(|error| format!("remove target: {error}"))?;
    ensure(
        !app_snapshot(&root, &options.app, &key)?.installed,
        "remove did not commit",
    )?;
    event(
        &mut evidence,
        "remove-confirmation",
        "installed package is absent and its tombstone is committed",
    );
    expect_install(&root, &options.app, &target, &key, "reinstall")?;
    event(
        &mut evidence,
        "reinstall",
        "same verified target package reinstalled successfully",
    );

    kobod::app_store::uninstall_using(&root, &options.app, &key)
        .map_err(|error| format!("prepare update baseline: {error}"))?;
    expect_install(&root, &options.app, &baseline, &key, "baseline install")?;
    expect_install(&root, &options.app, &target, &key, "update")?;
    event(
        &mut evidence,
        "update",
        "1.0.0 atomically activated to 1.1.0",
    );

    refresh(&root, &downgrade, &key)
        .map_err(|error| format!("refresh downgrade fixture: {error}"))?;
    let refused = plan(&root, &options.app, &downgrade, &key)?;
    ensure(!refused.install, "downgrade was offered for installation")?;
    ensure(
        app_snapshot(&root, &options.app, &key)?.version.as_deref() == Some("1.1.0"),
        "downgrade changed the installed version",
    )?;
    event(
        &mut evidence,
        "downgrade-refusal",
        "0.9.0 was refused and 1.1.0 remained active",
    );

    let before_failures = app_snapshot(&root, &options.app, &key)?;
    let mut truncated = target.clone();
    truncated
        .package
        .truncate(truncated.package.len().saturating_sub(1));
    expect_install_error(
        &root,
        &options.app,
        &truncated,
        &key,
        DeviceError::Integrity,
    )?;
    event(
        &mut evidence,
        "interrupted-download",
        "truncated package was rejected before activation",
    );

    let mut checksum = target.clone();
    checksum.package[0] ^= 1;
    expect_install_error(&root, &options.app, &checksum, &key, DeviceError::Integrity)?;
    event(
        &mut evidence,
        "checksum-failure",
        "package bytes not matching the signed catalog digest were rejected",
    );

    let mut bad_signature = target.clone();
    bad_signature.catalog_signature[0] = if bad_signature.catalog_signature[0] == b'0' {
        b'1'
    } else {
        b'0'
    };
    ensure(
        refresh(&root, &bad_signature, &key) == Err(DeviceError::Integrity),
        "bad catalog signature was accepted",
    )?;
    event(
        &mut evidence,
        "signature-failure",
        "altered detached catalog signature was rejected",
    );

    let malformed_catalog = signed_catalog_bytes(b"{}", &seed);
    let malformed_release = Release {
        catalog: b"{}".to_vec(),
        catalog_signature: malformed_catalog,
        ..target.clone()
    };
    ensure(
        refresh(&root, &malformed_release, &key) == Err(DeviceError::InvalidInput),
        "malformed signed catalog was accepted",
    )?;
    event(
        &mut evidence,
        "malformed-catalog",
        "signed but structurally invalid catalog was rejected",
    );

    let malformed_package = release_with_package(
        &options.app,
        "1.2.0",
        b"not-a-cobalt-bundle".to_vec(),
        &seed,
    )?;
    expect_install_error(
        &root,
        &options.app,
        &malformed_package,
        &key,
        DeviceError::Integrity,
    )?;
    event(
        &mut evidence,
        "malformed-package",
        "catalog-bound bytes with an invalid bundle were rejected",
    );
    ensure(
        app_snapshot(&root, &options.app, &key)? == before_failures,
        "a failed transaction changed the active application",
    )?;

    let lock_conflict = RunLock::acquire(&options.out);
    ensure(
        lock_conflict.is_err(),
        "a conflicting acceptance command acquired the same evidence directory",
    )?;
    event(
        &mut evidence,
        "command-conflict",
        "second command was refused by the evidence ownership lock",
    );

    let apps = root.join("apps");
    let previous = apps.join(format!("{}.prev", options.app));
    if previous.exists() {
        fs::remove_dir_all(&previous)
            .map_err(|error| format!("clear previous rollback fixture: {error}"))?;
    }
    fs::rename(apps.join(&options.app), &previous)
        .map_err(|error| format!("stage interrupted activation: {error}"))?;
    fs::create_dir_all(apps.join(format!("{}.next", options.app)))
        .map_err(|error| format!("stage incomplete next directory: {error}"))?;
    ensure(
        app_snapshot(&root, &options.app, &key)?.version.as_deref() == Some("1.1.0"),
        "interrupted activation did not restore the verified previous copy",
    )?;
    event(
        &mut evidence,
        "atomic-rollback",
        "interrupted activation recovered the verified previous copy",
    );

    install_direct(&root, &options.app, &launch_failure, &key)?;
    launch(&root, &options.app, &key, false)?;
    event(
        &mut evidence,
        "launch-failure",
        "non-zero fixture launch was detected and retained as failure evidence",
    );
    install_direct(&root, &options.app, &target, &key)?;
    launch(&root, &options.app, &key, true)?;
    mock_shot(&options.out.join("02-recovered-launch.png"), 212)?;
    event(
        &mut evidence,
        "launch-recovery",
        "known-good bytes were atomically restored after launch failure",
    );

    evidence.after = app_snapshot(&root, &options.app, &key)?;
    evidence.preservation_after = preservation(&root, &options.app)?;
    ensure(
        evidence.preservation_before == evidence.preservation_after,
        "unrelated apps, state, secrets, trust roots, data, or network ownership sentinels changed",
    )?;
    event(
        &mut evidence,
        "owner-data-preservation",
        "all protected digests match exactly; target state survived remove/reinstall",
    );
    evidence.marketing =
        create_mock_marketing(&options.out, &marketing_route, discover_encoder_tools())?;
    let marketing_complete = evidence.marketing.complete;
    evidence.events.push(Event {
        name: "marketing-capture".to_owned(),
        result: if marketing_complete {
            "passed"
        } else {
            "incomplete"
        }
        .to_owned(),
        detail: if marketing_complete {
            "scripted title, interaction, and result route produced validated MP4, WebM, GIF, and source frames"
        } else {
            "source frames and an exact ffmpeg command were retained, but encoded marketing artifacts are incomplete"
        }
        .to_owned(),
    });
    fs::write(
        options.out.join("logs.txt"),
        "mock: catalog verified\nmock: package activated\nmock: launch recovered\n",
    )
    .map_err(|error| format!("write redacted mock logs: {error}"))?;
    write_report(&options.out, &evidence)?;
    if !evidence.marketing.complete {
        return Err("marketing capture is incomplete; see report.json".to_owned());
    }
    println!(
        "beta Store mock acceptance passed; evidence: {}",
        options.out.display()
    );
    Ok(())
}

fn release(app: &str, version: &str, launch_ok: bool, seed: &[u8; 32]) -> Result<Release, String> {
    let status = if launch_ok { 0 } else { 23 };
    let binary = format!(
        "#!/bin/sh\n# deterministic Beta Store acceptance fixture\nprintf 'app={app} version={version}\\n'\nexit {status}\n"
    )
    .into_bytes();
    release_with_binary(app, version, &binary, seed)
}

fn release_with_package(
    app: &str,
    version: &str,
    package: Vec<u8>,
    seed: &[u8; 32],
) -> Result<Release, String> {
    let binary = b"malformed package placeholder".to_vec();
    let manifest = fixture_manifest(app, version, &binary)?;
    catalog_release(manifest, package, seed)
}

fn release_with_binary(
    app: &str,
    version: &str,
    binary: &[u8],
    seed: &[u8; 32],
) -> Result<Release, String> {
    let manifest = fixture_manifest(app, version, binary)?;
    let package = build_bundle(&manifest, binary, seed)
        .map_err(|error| format!("build fixture bundle: {error}"))?;
    catalog_release(manifest, package, seed)
}

fn fixture_manifest(app: &str, version: &str, binary: &[u8]) -> Result<Manifest, String> {
    Manifest::new_public(ManifestInput {
        id: app.to_owned(),
        display_name: "Beta acceptance fixture".to_owned(),
        short_label: "Beta fixture".to_owned(),
        summary: "Exercises the signed Beta Store transaction without owner data.".to_owned(),
        version: version.to_owned(),
        minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
        glyph: "check".to_owned(),
        capabilities: Vec::new(),
        binary_sha256: crate::sha256::hex_digest(binary),
        binary_bytes: binary.len() as u64,
    })
    .map_err(|error| format!("build fixture manifest: {error}"))
}

fn catalog_release(
    manifest: Manifest,
    package: Vec<u8>,
    seed: &[u8; 32],
) -> Result<Release, String> {
    let version = manifest.version().to_owned();
    let package_url = format!(
        "https://fixtures.invalid/releases/download/app-catalog-beta/{}-{version}.cobalt-app",
        manifest.id()
    );
    let binary_sha256 = manifest.binary_sha256().as_str().to_owned();
    let entry = CatalogEntry::new(CatalogEntryInput {
        manifest,
        package_url: package_url.clone(),
        package_sha256: crate::sha256::hex_digest(&package),
        package_bytes: package.len() as u64,
    })
    .map_err(|error| format!("build fixture catalog entry: {error}"))?;
    let catalog = Catalog::new(vec![entry])
        .map_err(|error| format!("build fixture catalog: {error}"))?
        .to_canonical_bytes();
    let catalog_signature = signed_catalog_bytes(&catalog, seed);
    Ok(Release {
        version,
        catalog,
        catalog_signature,
        package,
        package_url,
        binary_sha256,
    })
}

fn signed_catalog_bytes(catalog: &[u8], seed: &[u8; 32]) -> Vec<u8> {
    format!(
        "{}\n",
        sign(catalog, seed).expect("fixture seed must be accepted")
    )
    .into_bytes()
}

fn verify_release(release: &Release, key: &Ed25519PublicKey) -> Result<(), String> {
    let signature =
        DetachedSignature::from_hex(String::from_utf8_lossy(&release.catalog_signature).trim())
            .map_err(|error| format!("parse fixture catalog signature: {error}"))?;
    verify(&release.catalog, &signature, key)
        .map_err(|error| format!("verify fixture catalog signature: {error}"))?;
    let catalog = Catalog::parse_public(&release.catalog)
        .map_err(|error| format!("parse catalog: {error}"))?;
    ensure(
        catalog.to_canonical_bytes() == release.catalog,
        "fixture catalog is not canonical",
    )?;
    let entry = catalog
        .entries()
        .first()
        .ok_or("fixture catalog is empty")?;
    ensure(
        entry.package_url() == release.package_url,
        "fixture package URL drifted",
    )?;
    ensure(
        entry.package_sha256().as_str() == crate::sha256::hex_digest(&release.package),
        "fixture package digest drifted",
    )?;
    let bundle = parse_public_bundle(&release.package, key)
        .map_err(|error| format!("verify fixture package: {error}"))?;
    ensure(
        bundle.manifest().binary_sha256().as_str() == release.binary_sha256,
        "fixture binary digest drifted",
    )
}

fn refresh(
    root: &Path,
    release: &Release,
    key: &Ed25519PublicKey,
) -> Result<Vec<kobo_protocol::AppInfo>, DeviceError> {
    kobod::app_store::refresh_using(root, UpdateChannel::Beta, key, |url, _| release.fetch(url))
}

fn plan(
    root: &Path,
    app: &str,
    release: &Release,
    key: &Ed25519PublicKey,
) -> Result<kobod::app_store::RemoteInstallPlan, String> {
    refresh(root, release, key).map_err(|error| format!("refresh fixture catalog: {error}"))?;
    kobod::app_store::prepare_remote_install_using(root, app, UpdateChannel::Beta, key, |url, _| {
        release.fetch(url)
    })
    .map_err(|error| format!("plan fixture install: {error}"))
}

fn expect_install(
    root: &Path,
    app: &str,
    release: &Release,
    key: &Ed25519PublicKey,
    stage: &str,
) -> Result<(), String> {
    let plan = plan(root, app, release, key)?;
    ensure(plan.install, &format!("{stage} was not offered"))?;
    install_direct(root, app, release, key)?;
    let snapshot = app_snapshot(root, app, key)?;
    ensure(
        snapshot.version.as_deref() == Some(release.version.as_str()),
        &format!("{stage} activated the wrong version"),
    )
}

fn install_direct(
    root: &Path,
    app: &str,
    release: &Release,
    key: &Ed25519PublicKey,
) -> Result<(), String> {
    refresh(root, release, key).map_err(|error| format!("refresh before install: {error}"))?;
    kobod::app_store::install_using(root, app, UpdateChannel::Beta, key, |url, _| {
        release.fetch(url)
    })
    .map_err(|error| format!("install fixture package: {error}"))
}

fn expect_install_error(
    root: &Path,
    app: &str,
    release: &Release,
    key: &Ed25519PublicKey,
    expected: DeviceError,
) -> Result<(), String> {
    refresh(root, release, key).map_err(|error| format!("refresh failure fixture: {error}"))?;
    let result = kobod::app_store::install_using(root, app, UpdateChannel::Beta, key, |url, _| {
        release.fetch(url)
    });
    ensure(
        result == Err(expected),
        &format!("failure fixture returned {result:?}, expected {expected:?}"),
    )
}

impl Release {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, DeviceError> {
        match url {
            kobod::app_store::BETA_CATALOG_URL => Ok(self.catalog.clone()),
            kobod::app_store::BETA_CATALOG_SIGNATURE_URL => Ok(self.catalog_signature.clone()),
            value if value == self.package_url => Ok(self.package.clone()),
            _ => Err(DeviceError::NotFound),
        }
    }
}

fn app_snapshot(root: &Path, app: &str, key: &Ed25519PublicKey) -> Result<AppSnapshot, String> {
    let installed = kobod::app_store::installed_using(root, key)
        .map_err(|error| format!("read installed apps: {error}"))?;
    let Some(info) = installed.iter().find(|candidate| candidate.id == app) else {
        return Ok(AppSnapshot {
            installed: false,
            version: None,
            binary_sha256: None,
        });
    };
    let binary = kobod::app_store::resolve_using(root, app, key)
        .map_err(|error| format!("resolve installed app: {error}"))?;
    let bytes = fs::read(binary).map_err(|error| format!("read installed binary: {error}"))?;
    Ok(AppSnapshot {
        installed: true,
        version: info.installed_version.clone(),
        binary_sha256: Some(crate::sha256::hex_digest(&bytes)),
    })
}

fn launch(
    root: &Path,
    app: &str,
    key: &Ed25519PublicKey,
    should_succeed: bool,
) -> Result<(), String> {
    let binary = kobod::app_store::resolve_using(root, app, key)?;
    let status = Command::new(&binary)
        .arg("--beta-store-smoke")
        .status()
        .map_err(|error| format!("launch {}: {error}", binary.display()))?;
    ensure(
        status.success() == should_succeed,
        "fixture launch returned an unexpected status",
    )
}

fn seed_owner_state(root: &Path, app: &str) -> Result<(), String> {
    for (relative, contents) in [
        (
            "apps/unrelated/owner.bin",
            b"unrelated application bytes".as_slice(),
        ),
        ("state/unrelated/preference", b"owner preference"),
        ("secrets/provider", b"fixture-owner-secret"),
        ("trust/private-ca.pem", b"fixture-owner-trust-root"),
        ("data/unrelated/library", b"owner library sentinel"),
        ("network-owner.conf", b"wifi ownership sentinel"),
    ] {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create owner-state fixture: {error}"))?;
        }
        fs::write(path, contents).map_err(|error| format!("write owner-state fixture: {error}"))?;
    }
    let target = root.join("state").join(app).join("position");
    fs::create_dir_all(target.parent().expect("target state has a parent"))
        .map_err(|error| format!("create target state fixture: {error}"))?;
    fs::write(target, b"opaque owner state")
        .map_err(|error| format!("write target state fixture: {error}"))
}

fn preservation(root: &Path, app: &str) -> Result<Preservation, String> {
    let mut unrelated = Vec::new();
    collect_digest_records(root, root, Some(app), &mut unrelated)?;
    unrelated.sort();
    let state = digest_excluding_child(&root.join("state"), app)?;
    let target_state = target_state_digest(root, app)?;
    Ok(Preservation {
        unrelated: crate::sha256::hex_digest(unrelated.join("\n").as_bytes()),
        state,
        target_state,
    })
}

fn digest_excluding_child(path: &Path, child: &str) -> Result<String, String> {
    let mut records = Vec::new();
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(crate::sha256::hex_digest(b"absent"));
        }
        Err(error) => return Err(format!("read protected path {}: {error}", path.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read protected entry: {error}"))?;
        if entry.file_name() == child {
            continue;
        }
        collect_digest_records(path, &entry.path(), None, &mut records)?;
    }
    records.sort();
    Ok(crate::sha256::hex_digest(records.join("\n").as_bytes()))
}

fn target_state_digest(root: &Path, app: &str) -> Result<String, String> {
    let state = digest_path(&root.join("state").join(app))?;
    let data = digest_path(&root.join("data").join(app))?;
    Ok(crate::sha256::hex_digest(
        format!("{state}\n{data}\n").as_bytes(),
    ))
}

fn collect_digest_records(
    root: &Path,
    path: &Path,
    excluded_app: Option<&str>,
    records: &mut Vec<String>,
) -> Result<(), String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read protected path {}: {error}", path.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read protected entry: {error}"))?;
        let child = entry.path();
        let relative = child
            .strip_prefix(root)
            .map_err(|_| "protected path escaped its root".to_owned())?;
        let relative_text = relative.to_string_lossy();
        if relative_text == "store" || relative_text.starts_with("store/") {
            continue;
        }
        if let Some(app) = excluded_app {
            let excluded = ["apps", "state", "data"].iter().any(|parent| {
                let app_prefix = format!("{parent}/{app}");
                relative_text == app_prefix
                    || relative_text.starts_with(&format!("{app_prefix}."))
                    || relative_text.starts_with(&format!("{app_prefix}/"))
            });
            if excluded {
                continue;
            }
        }
        let metadata = fs::symlink_metadata(&child)
            .map_err(|error| format!("read protected metadata: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "protected fixture contains symlink {}",
                relative.display()
            ));
        }
        if metadata.is_dir() {
            collect_digest_records(root, &child, excluded_app, records)?;
        } else if metadata.is_file() {
            let bytes =
                fs::read(&child).map_err(|error| format!("read protected fixture: {error}"))?;
            records.push(format!(
                "{}\t{}\t{}",
                relative_text,
                bytes.len(),
                crate::sha256::hex_digest(&bytes)
            ));
        }
    }
    Ok(())
}

fn digest_path(path: &Path) -> Result<String, String> {
    let root = path.parent().unwrap_or(path);
    let mut records = Vec::new();
    collect_digest_records(root, path, None, &mut records)?;
    records.sort();
    Ok(crate::sha256::hex_digest(records.join("\n").as_bytes()))
}

fn mock_shot(path: &Path, grey: u8) -> Result<(), String> {
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 96;
    let frame = vec![grey; (WIDTH * HEIGHT) as usize];
    let png = kobo_image::encode_png_grey(WIDTH, HEIGHT, &frame)
        .map_err(|error| format!("encode mock screenshot: {error}"))?;
    fs::write(path, png).map_err(|error| format!("write mock screenshot: {error}"))
}

impl MarketingEvidence {
    fn pending(route: &MarketingRoute) -> Self {
        Self {
            complete: false,
            reason: Some("marketing capture has not run".to_owned()),
            title: route.title.clone(),
            interaction: route.interaction.clone(),
            result: route.result.clone(),
            width: 0,
            height: 0,
            frame_count: 0,
            duration_ms: 0,
            fps: MARKETING_FPS,
            reproducible_command: "marketing/ffmpeg-command.sh".to_owned(),
            artifacts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct EncoderTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

fn discover_encoder_tools() -> Result<EncoderTools, String> {
    let ffmpeg = command_available("ffmpeg")
        .then(|| PathBuf::from("ffmpeg"))
        .ok_or("ffmpeg is not available on PATH")?;
    let ffprobe = command_available("ffprobe")
        .then(|| PathBuf::from("ffprobe"))
        .ok_or("ffprobe is not available on PATH")?;
    Ok(EncoderTools { ffmpeg, ffprobe })
}

fn command_available(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn read_marketing_route(path: &Path) -> Result<MarketingRoute, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("read marketing route {}: {error}", path.display()))?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() != 6
        || lines[0] != "cobalt-beta-marketing-route 1"
        || lines[1] != "privacy public-demo-no-owner-data"
    {
        return Err(format!(
            "{} is not a canonical Beta marketing route",
            path.display()
        ));
    }
    let title = route_field(lines[2], "title")?;
    let interaction = route_field(lines[3], "interaction")?;
    let result = route_field(lines[4], "result")?;
    let timing = route_field(lines[5], "timing")?;
    let interaction_parts = interaction.split('|').collect::<Vec<_>>();
    if interaction_parts.len() != 3 {
        return Err("interaction must be LABEL|X|Y".to_owned());
    }
    let interaction_label = public_route_label(interaction_parts[0], "interaction")?;
    let x = route_number(interaction_parts[1], "interaction X", 0, 4095)?;
    let y = route_number(interaction_parts[2], "interaction Y", 0, 4095)?;
    let timing_parts = timing.split('|').collect::<Vec<_>>();
    if timing_parts.len() != 2 {
        return Err("timing must be TITLE_MS|RESULT_MS".to_owned());
    }
    let title_ms = u64::from(route_number(timing_parts[0], "title timing", 500, 5_000)?);
    let result_ms = u64::from(route_number(timing_parts[1], "result timing", 500, 5_000)?);
    if title_ms + result_ms > 8_000 {
        return Err("marketing route is longer than the eight-second ceiling".to_owned());
    }
    Ok(MarketingRoute {
        title: public_route_label(&title, "title")?,
        interaction: interaction_label,
        result: public_route_label(&result, "result")?,
        x,
        y,
        title_ms,
        result_ms,
    })
}

fn route_field(line: &str, name: &str) -> Result<String, String> {
    line.strip_prefix(&format!("{name} "))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("marketing route has no {name}"))
}

fn public_route_label(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let sensitive = [
        "password",
        "secret",
        "token",
        "credential",
        "ssid",
        "wi-fi",
        "wifi",
        "email",
        "account",
        "notification",
        "private",
    ];
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b' ' | b'-' | b'_' | b'.' | b':' | b'/' | b'(' | b')')
        })
        || sensitive.iter().any(|word| lower.contains(word))
        || looks_like_ipv4(trimmed)
        || looks_like_mac(trimmed)
    {
        return Err(format!("{field} is not a public-safe marketing label"));
    }
    Ok(trimmed.to_owned())
}

fn route_number(value: &str, field: &str, minimum: u32, maximum: u32) -> Result<u32, String> {
    let number = value
        .parse::<u32>()
        .map_err(|_| format!("{field} must be a whole number"))?;
    if (minimum..=maximum).contains(&number) {
        Ok(number)
    } else {
        Err(format!("{field} must be between {minimum} and {maximum}"))
    }
}

fn create_mock_marketing(
    evidence_root: &Path,
    route: &MarketingRoute,
    tools: Result<EncoderTools, String>,
) -> Result<MarketingEvidence, String> {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 240;
    let directory = evidence_root.join("marketing");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create mock marketing directory: {error}"))?;
    write_marketing_route(&directory, route)?;
    let stages = [
        (0_u64, 238_u8),
        (route.title_ms, 164_u8),
        (route.title_ms + route.result_ms, 76_u8),
    ];
    let mut timings = String::new();
    for (index, (millis, grey)) in stages.iter().enumerate() {
        let mut frame = vec![*grey; (WIDTH * HEIGHT) as usize];
        let band_start = (index + 1) * frame.len() / 5;
        let band_end = (band_start + frame.len() / 12).min(frame.len());
        frame[band_start..band_end].fill(255_u8.saturating_sub(*grey));
        let png = kobo_image::encode_png_grey(WIDTH, HEIGHT, &frame)
            .map_err(|error| format!("encode marketing frame: {error}"))?;
        let name = format!("frame-{index:04}.png");
        fs::write(directory.join(&name), png)
            .map_err(|error| format!("write marketing frame: {error}"))?;
        let _ = writeln!(timings, "{name} {millis}");
    }
    fs::write(directory.join("timings.txt"), timings)
        .map_err(|error| format!("write marketing timings: {error}"))?;
    finalize_marketing(
        evidence_root,
        route,
        WIDTH,
        HEIGHT,
        route.title_ms + route.result_ms + 1_000,
        tools,
    )
}

fn finalize_marketing(
    evidence_root: &Path,
    route: &MarketingRoute,
    expected_width: u32,
    expected_height: u32,
    capture_duration_ms: u64,
    tools: Result<EncoderTools, String>,
) -> Result<MarketingEvidence, String> {
    let directory = evidence_root.join("marketing");
    remove_encoded_marketing(&directory);
    let timings = validate_marketing_frames(&directory, expected_width, expected_height)?;
    validate_marketing_milestones(&timings, route)?;
    let route_duration = route.title_ms + route.result_ms;
    if capture_duration_ms < route_duration
        || capture_duration_ms > route_duration.saturating_add(5_000)
        || !(1_000..=13_500).contains(&capture_duration_ms)
        || timings
            .last()
            .is_some_and(|(_, millis)| *millis >= capture_duration_ms)
    {
        return Err("marketing capture duration does not cover the scripted route".to_owned());
    }
    let concat = write_marketing_concat(&directory, &timings, capture_duration_ms)?;
    let command = marketing_command_text();
    let command_path = directory.join("ffmpeg-command.sh");
    fs::write(&command_path, &command)
        .map_err(|error| format!("write reproducible ffmpeg command: {error}"))?;

    let mut evidence = MarketingEvidence {
        complete: false,
        reason: None,
        title: route.title.clone(),
        interaction: route.interaction.clone(),
        result: route.result.clone(),
        width: expected_width,
        height: expected_height,
        frame_count: timings.len(),
        duration_ms: capture_duration_ms,
        fps: MARKETING_FPS,
        reproducible_command: relative_path(evidence_root, &command_path)?,
        artifacts: Vec::new(),
    };
    let tools = match tools {
        Ok(tools) => tools,
        Err(reason) => {
            evidence.reason = Some(reason);
            evidence.artifacts = collect_marketing_artifacts(evidence_root, &directory)?;
            return Ok(evidence);
        }
    };
    if let Err(error) = encode_marketing(&directory, &concat, &tools) {
        remove_encoded_marketing(&directory);
        evidence.reason = Some(error);
        evidence.artifacts = collect_marketing_artifacts(evidence_root, &directory)?;
        return Ok(evidence);
    }
    for name in ["marketing.mp4", "marketing.webm", "marketing.gif"] {
        if let Err(error) = validate_encoded_marketing(
            &directory.join(name),
            expected_width,
            expected_height,
            capture_duration_ms,
            &tools.ffprobe,
        ) {
            remove_encoded_marketing(&directory);
            evidence.reason = Some(error);
            evidence.artifacts = collect_marketing_artifacts(evidence_root, &directory)?;
            return Ok(evidence);
        }
    }
    evidence.complete = true;
    evidence.artifacts = collect_marketing_artifacts(evidence_root, &directory)?;
    Ok(evidence)
}

fn validate_marketing_milestones(
    timings: &[(String, u64)],
    route: &MarketingRoute,
) -> Result<(), String> {
    let interaction_floor = route.title_ms.saturating_sub(250);
    let interaction = timings
        .iter()
        .position(|(_, millis)| *millis >= interaction_floor)
        .ok_or("marketing frames never reached the scripted interaction")?;
    if interaction == 0 {
        return Err("marketing capture has no distinct title frame".to_owned());
    }
    let interaction_millis = timings[interaction].1;
    let result_floor = interaction_millis.saturating_add(100);
    if !timings
        .iter()
        .skip(interaction + 1)
        .any(|(_, millis)| *millis >= result_floor)
    {
        return Err("marketing frames never reached a distinct result state".to_owned());
    }
    Ok(())
}

fn validate_marketing_frames(
    directory: &Path,
    expected_width: u32,
    expected_height: u32,
) -> Result<Vec<(String, u64)>, String> {
    if expected_width == 0
        || expected_height == 0
        || expected_width % 2 != 0
        || expected_height % 2 != 0
    {
        return Err("marketing dimensions must be non-zero and even".to_owned());
    }
    let text = fs::read_to_string(directory.join("timings.txt"))
        .map_err(|error| format!("read marketing timings: {error}"))?;
    let mut timings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let (name, millis) = line
            .split_once(' ')
            .ok_or("malformed marketing timing line")?;
        let expected = format!("frame-{index:04}.png");
        if name != expected {
            return Err(format!(
                "marketing frame continuity broke at {name:?}; expected {expected:?}"
            ));
        }
        let millis = millis
            .parse::<u64>()
            .map_err(|_| "marketing timestamp is not a whole number".to_owned())?;
        if timings
            .last()
            .is_some_and(|(_, previous)| millis <= *previous)
        {
            return Err("marketing frame timestamps are not strictly increasing".to_owned());
        }
        let bytes = fs::read(directory.join(name))
            .map_err(|error| format!("read marketing frame {name}: {error}"))?;
        let picture = kobo_image::decode(&bytes)
            .map_err(|error| format!("decode marketing frame {name}: {error}"))?;
        if picture.width() != expected_width || picture.height() != expected_height {
            return Err(format!(
                "marketing frame {name} is {}x{}, expected {expected_width}x{expected_height}",
                picture.width(),
                picture.height()
            ));
        }
        timings.push((name.to_owned(), millis));
    }
    if timings.len() < 3 {
        return Err("marketing capture needs title, interaction, and result frames".to_owned());
    }
    if timings[0].1 > 1_000 || timings.last().is_some_and(|(_, millis)| *millis > 12_000) {
        return Err("marketing capture duration is outside its concise bound".to_owned());
    }
    Ok(timings)
}

fn write_marketing_concat(
    directory: &Path,
    timings: &[(String, u64)],
    capture_duration_ms: u64,
) -> Result<PathBuf, String> {
    let mut text = String::from("ffconcat version 1.0\n");
    for (index, (name, millis)) in timings.iter().enumerate() {
        let next = timings
            .get(index + 1)
            .map_or(capture_duration_ms, |(_, next)| *next);
        let duration = next.saturating_sub(*millis).max(100);
        let seconds = duration / 1_000;
        let milliseconds = duration % 1_000;
        let _ = writeln!(text, "file '{name}'\nduration {seconds}.{milliseconds:03}");
    }

    if let Some((name, _)) = timings.last() {
        let _ = writeln!(text, "file '{name}'");
    }
    let path = directory.join("frames.ffconcat");
    fs::write(&path, text).map_err(|error| format!("write marketing concat list: {error}"))?;
    Ok(path)
}

fn remove_encoded_marketing(directory: &Path) {
    for name in [
        "recording.mp4",
        "marketing.mp4",
        "marketing.webm",
        "marketing.gif",
    ] {
        let _ignored = fs::remove_file(directory.join(name));
    }
}

fn marketing_command_text() -> String {
    format!(
        "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\n\
         ffmpeg -y -f concat -safe 0 -i frames.ffconcat -vf 'fps={MARKETING_FPS}' \
           -c:v libx264 -preset slow -crf 28 -pix_fmt yuv420p -movflags +faststart marketing.mp4\n\
         ffmpeg -y -f concat -safe 0 -i frames.ffconcat -vf 'fps={MARKETING_FPS}' \
           -c:v libvpx-vp9 -crf 36 -b:v 0 -pix_fmt yuv420p marketing.webm\n\
         ffmpeg -y -f concat -safe 0 -i frames.ffconcat \
           -filter_complex '[0:v]fps={MARKETING_FPS},split[a][b];[a]palettegen=max_colors=48:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle' \
           marketing.gif\n",
    )
}

fn write_marketing_route(directory: &Path, route: &MarketingRoute) -> Result<(), String> {
    let text = format!(
        "cobalt-beta-marketing-route 1\n\
         privacy public-demo-no-owner-data\n\
         title {}\n\
         interaction {}|{}|{}\n\
         result {}\n\
         timing {}|{}\n",
        route.title,
        route.interaction,
        route.x,
        route.y,
        route.result,
        route.title_ms,
        route.result_ms
    );
    fs::write(directory.join("route.txt"), text)
        .map_err(|error| format!("write marketing route evidence: {error}"))
}

fn encode_marketing(directory: &Path, concat: &Path, tools: &EncoderTools) -> Result<(), String> {
    let commands: [(&str, &[&str]); 3] = [
        (
            "marketing.mp4",
            &[
                "-vf",
                "fps=4",
                "-c:v",
                "libx264",
                "-preset",
                "slow",
                "-crf",
                "28",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ],
        ),
        (
            "marketing.webm",
            &[
                "-vf",
                "fps=4",
                "-c:v",
                "libvpx-vp9",
                "-crf",
                "36",
                "-b:v",
                "0",
                "-pix_fmt",
                "yuv420p",
            ],
        ),
        (
            "marketing.gif",
            &[
                "-filter_complex",
                "[0:v]fps=4,split[a][b];[a]palettegen=max_colors=48:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle",
            ],
        ),
    ];
    for (output, extra) in commands {
        let status = Command::new(&tools.ffmpeg)
            .current_dir(directory)
            .args(["-y", "-f", "concat", "-safe", "0", "-i"])
            .arg(
                concat
                    .file_name()
                    .ok_or("marketing concat path has no file name")?,
            )
            .args(extra)
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("run ffmpeg for {output}: {error}"))?;
        if !status.success() {
            return Err(format!("ffmpeg could not produce {output}"));
        }
    }
    Ok(())
}

fn validate_encoded_marketing(
    path: &Path,
    expected_width: u32,
    expected_height: u32,
    expected_duration_ms: u64,
    ffprobe: &Path,
) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("read {} metadata: {error}", path.display()))?;
    if metadata.len() == 0 || metadata.len() > MARKETING_MAX_BYTES {
        return Err(format!(
            "{} has invalid marketing size {}",
            path.display(),
            metadata.len()
        ));
    }
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .map_err(|error| format!("run ffprobe on {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!("ffprobe rejected {}", path.display()));
    }
    let values = fields(&String::from_utf8_lossy(&output.stdout));
    let width = values
        .get("width")
        .and_then(|value| value.parse::<u32>().ok());
    let height = values
        .get("height")
        .and_then(|value| value.parse::<u32>().ok());
    let duration_ms = values
        .get("duration")
        .and_then(|value| parse_seconds_millis(value));
    if width != Some(expected_width) || height != Some(expected_height) {
        return Err(format!(
            "{} dimensions do not match source frames",
            path.display()
        ));
    }
    let duration_ms = duration_ms.ok_or_else(|| format!("{} has no duration", path.display()))?;
    if duration_ms.abs_diff(expected_duration_ms) > 1_500
        || !(1_000..=13_500).contains(&duration_ms)
    {
        return Err(format!(
            "{} duration is outside the accepted bound",
            path.display()
        ));
    }
    Ok(())
}

fn parse_seconds_millis(value: &str) -> Option<u64> {
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    let seconds = seconds.parse::<u64>().ok()?;
    let mut milliseconds = 0_u64;
    for (index, byte) in fraction.bytes().take(3).enumerate() {
        if !byte.is_ascii_digit() {
            return None;
        }
        let digit = u64::from(byte - b'0');
        milliseconds += digit
            * match index {
                0 => 100,
                1 => 10,
                _ => 1,
            };
    }
    Some(seconds.saturating_mul(1_000).saturating_add(milliseconds))
}

fn collect_marketing_artifacts(
    evidence_root: &Path,
    directory: &Path,
) -> Result<Vec<MarketingArtifact>, String> {
    let mut paths = fs::read_dir(directory)
        .map_err(|error| format!("read marketing artifacts: {error}"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read marketing artifact entry: {error}"))?;
    paths.sort();
    let mut artifacts = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let kind = if extension == "png" {
            "frame"
        } else if extension == "mp4" {
            "mp4"
        } else if extension == "webm" {
            "webm"
        } else if extension == "gif" {
            "gif"
        } else if extension == "sh" {
            "reproducible-command"
        } else {
            "metadata"
        };
        artifacts.push(MarketingArtifact {
            kind: kind.to_owned(),
            path: relative_path(evidence_root, &path)?,
            sha256: crate::sha256::hex_digest(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    Ok(artifacts)
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| format!("{} escaped the evidence directory", path.display()))
}

fn event(evidence: &mut Evidence, name: &str, detail: &str) {
    evidence.events.push(Event {
        name: name.to_owned(),
        result: "passed".to_owned(),
        detail: detail.to_owned(),
    });
}

fn ensure(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn read_seed(path: &Path) -> Result<[u8; 32], String> {
    let value = fs::read_to_string(path)
        .map_err(|error| format!("read fixture seed {}: {error}", path.display()))?;
    decode_hex_32(value.trim()).ok_or_else(|| {
        format!(
            "{} must contain exactly 64 lowercase hexadecimal characters",
            path.display()
        )
    })
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut out = [0_u8; 32];
    for (slot, pair) in out.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *slot = (hex(pair[0])? << 4) | hex(pair[1])?;
    }
    Some(out)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn write_report(out: &Path, evidence: &Evidence) -> Result<(), String> {
    let mut json = String::new();
    json.push_str("{\n  \"format_version\":1,\n  \"channel\":\"beta\",\n  \"mode\":");
    push_json(&mut json, evidence.mode);
    json.push_str(",\n  \"app_id\":");
    push_json(&mut json, &evidence.app);
    json.push_str(",\n  \"catalog_url\":");
    push_json(&mut json, &evidence.catalog_url);
    json.push_str(",\n  \"catalog_sha256\":");
    push_json(&mut json, &evidence.catalog_sha256);
    json.push_str(",\n  \"catalog_signature\":");
    push_json(&mut json, &evidence.catalog_signature);
    json.push_str(",\n  \"public_key_fingerprint\":");
    push_json(&mut json, &evidence.public_key_fingerprint);
    json.push_str(",\n  \"package_sha256\":");
    push_json(&mut json, &evidence.package_sha256);
    let _ = write!(
        json,
        ",\n  \"package_bytes\":{},\n  \"before\":{},\n  \"after\":{},\n  \"preservation_before\":{},\n  \"preservation_after\":{},\n  \"marketing\":{},\n  \"events\":[",
        evidence.package_bytes,
        snapshot_json(&evidence.before),
        snapshot_json(&evidence.after),
        preservation_json(&evidence.preservation_before),
        preservation_json(&evidence.preservation_after),
        marketing_json(&evidence.marketing),
    );
    for (index, item) in evidence.events.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"name\":");
        push_json(&mut json, &item.name);
        json.push_str(",\"result\":");
        push_json(&mut json, &item.result);
        json.push_str(",\"detail\":");
        push_json(&mut json, &item.detail);
        json.push('}');
    }
    json.push_str("\n  ]\n}\n");
    fs::write(out.join("report.json"), json)
        .map_err(|error| format!("write acceptance report: {error}"))
}

fn marketing_json(value: &MarketingEvidence) -> String {
    let mut out = String::from("{\"complete\":");
    out.push_str(if value.complete { "true" } else { "false" });
    out.push_str(",\"reason\":");
    push_optional_json(&mut out, value.reason.as_deref());
    out.push_str(",\"title\":");
    push_json(&mut out, &value.title);
    out.push_str(",\"interaction\":");
    push_json(&mut out, &value.interaction);
    out.push_str(",\"result\":");
    push_json(&mut out, &value.result);
    let _ = write!(
        out,
        ",\"width\":{},\"height\":{},\"frame_count\":{},\"duration_ms\":{},\"fps\":{}",
        value.width, value.height, value.frame_count, value.duration_ms, value.fps
    );
    out.push_str(",\"reproducible_command\":");
    push_json(&mut out, &value.reproducible_command);
    out.push_str(",\"artifacts\":[");
    for (index, artifact) in value.artifacts.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"kind\":");
        push_json(&mut out, &artifact.kind);
        out.push_str(",\"path\":");
        push_json(&mut out, &artifact.path);
        out.push_str(",\"sha256\":");
        push_json(&mut out, &artifact.sha256);
        let _ = write!(out, ",\"bytes\":{}", artifact.bytes);
        out.push('}');
    }
    out.push_str("]}");
    out
}

fn snapshot_json(snapshot: &AppSnapshot) -> String {
    let mut out = String::from("{\"installed\":");
    out.push_str(if snapshot.installed { "true" } else { "false" });
    out.push_str(",\"version\":");
    push_optional_json(&mut out, snapshot.version.as_deref());
    out.push_str(",\"binary_sha256\":");
    push_optional_json(&mut out, snapshot.binary_sha256.as_deref());
    out.push('}');
    out
}

fn preservation_json(value: &Preservation) -> String {
    let mut out = String::from("{\"unrelated_digest\":");
    push_json(&mut out, &value.unrelated);
    out.push_str(",\"state_digest\":");
    push_json(&mut out, &value.state);
    out.push_str(",\"target_state_digest\":");
    push_json(&mut out, &value.target_state);
    out.push('}');
    out
}

fn push_optional_json(out: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        push_json(out, value);
    } else {
        out.push_str("null");
    }
}

fn push_json(out: &mut String, value: &str) {
    kobo_json::escape_into(value, out);
}

fn run_physical(options: &Options, device: &str) -> Result<(), String> {
    let mut panel_session = None;
    let result = run_physical_inner(options, device, &mut panel_session);
    if let Err(error) = &result {
        if panel_session
            .as_ref()
            .is_some_and(|session| crate::panel::stop_store_app(session).is_err())
        {
            let remote = format!("root@{device}");
            let _ignored =
                crate::panel::run_remote_shell_waking(&remote, "exit\n", Duration::from_secs(15));
            let _ignored =
                crate::run_remote_shell(&remote, "sync\nreboot\n", Duration::from_secs(15));
        }
        let _ignored = fs::write(
            options.out.join("failure.txt"),
            format!("{}\n", redact(error)),
        );
    }
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "ordered device safety, evidence, and recovery steps form one attended transaction"
)]
fn run_physical_inner(
    options: &Options,
    device: &str,
    panel_session: &mut Option<crate::panel::StorePanelSession>,
) -> Result<(), String> {
    let identity = remote_beta(device, &["identity"], false)?;
    let identity_fields = fields(&identity);
    let marketing_route_path = options
        .marketing_route
        .as_deref()
        .ok_or("attended mode has no marketing route")?;
    let marketing_route = read_marketing_route(marketing_route_path)?;
    let panel_width = identity_fields
        .get("panel_width")
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("device did not report its panel width")?;
    let panel_height = identity_fields
        .get("panel_height")
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or("device did not report its panel height")?;
    ensure(
        marketing_route.x < panel_width && marketing_route.y < panel_height,
        "marketing interaction is outside the confirmed device panel",
    )?;
    let expected_protocol = kobo_protocol::VERSION.to_string();
    for (name, expected) in [
        ("profile", options.expected_profile.as_deref()),
        ("cobalt", options.expected_cobalt.as_deref()),
        ("firmware", options.expected_firmware.as_deref()),
        ("protocol", Some(expected_protocol.as_str())),
        ("channel", Some("beta")),
    ] {
        let expected = expected.ok_or_else(|| format!("missing expected {name}"))?;
        ensure(
            identity_fields.get(name).map(String::as_str) == Some(expected),
            &format!(
                "device {name} mismatch: expected {expected:?}, found {:?}",
                identity_fields.get(name)
            ),
        )?;
    }
    ensure_no_panel_session(device)?;

    let catalog_url = options
        .beta_catalog
        .as_deref()
        .ok_or("attended mode has no Beta catalog URL")?;
    let catalog = kobo_net::fetch(catalog_url, 512 * 1024)
        .map_err(|error| format!("download Beta catalog: {error}"))?;
    let signature = kobo_net::fetch(&format!("{catalog_url}.sig"), 1024)
        .map_err(|error| format!("download Beta catalog signature: {error}"))?;
    let public = Ed25519PublicKey::from_hex(PUBLIC_RELEASE_KEY_HEX)
        .map_err(|error| format!("read built-in Store key: {error}"))?;
    let detached = DetachedSignature::from_hex(String::from_utf8_lossy(&signature).trim())
        .map_err(|error| format!("parse Beta catalog signature: {error}"))?;
    verify(&catalog, &detached, &public)
        .map_err(|error| format!("verify Beta catalog signature: {error}"))?;
    let parsed =
        Catalog::parse_public(&catalog).map_err(|error| format!("parse Beta catalog: {error}"))?;
    ensure(
        parsed.to_canonical_bytes() == catalog,
        "Beta catalog is not canonical",
    )?;
    let entry = parsed
        .entries()
        .iter()
        .find(|entry| entry.manifest().id() == options.app)
        .ok_or_else(|| format!("{} is absent from the signed Beta catalog", options.app))?;
    let prefix = "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog-beta/";
    ensure(
        entry.package_url().starts_with(prefix),
        "Beta package URL escaped the isolated beta release",
    )?;
    let package = kobo_net::fetch(
        entry.package_url(),
        u32::try_from(entry.package_bytes()).map_err(|_| "Beta package is too large")?,
    )
    .map_err(|error| format!("download Beta package: {error}"))?;
    ensure(
        package.len() as u64 == entry.package_bytes()
            && crate::sha256::hex_digest(&package) == entry.package_sha256().as_str(),
        "Beta package bytes do not match the signed catalog",
    )?;
    let bundle = parse_public_bundle(&package, &public)
        .map_err(|error| format!("verify Beta package: {error}"))?;
    ensure(
        bundle.manifest() == entry.manifest(),
        "package manifest differs from catalog",
    )?;

    fs::write(options.out.join("catalog.json"), &catalog)
        .map_err(|error| format!("write catalog evidence: {error}"))?;
    fs::write(options.out.join("catalog.json.sig"), &signature)
        .map_err(|error| format!("write signature evidence: {error}"))?;
    fs::write(
        options.out.join(format!("{}.cobalt-app", options.app)),
        &package,
    )
    .map_err(|error| format!("write package evidence: {error}"))?;

    let before = physical_status(device, &options.app)?;
    let preservation_before = physical_preservation(device, &options.app)?;
    crate::shot_command(&[
        "--device".to_owned(),
        device.to_owned(),
        "--out".to_owned(),
        options.out.join("01-before.png").display().to_string(),
    ])?;
    remote_beta(device, &["refresh"], true)?;
    let cached = fields(&remote_beta(device, &["catalog-digest"], false)?);
    let host_catalog_sha256 = crate::sha256::hex_digest(&catalog);
    let host_signature_sha256 = crate::sha256::hex_digest(&signature);
    ensure(
        cached.get("catalog_sha256").map(String::as_str) == Some(host_catalog_sha256.as_str())
            && cached.get("signature_sha256").map(String::as_str)
                == Some(host_signature_sha256.as_str()),
        "device cached catalog/signature differ from the archived evidence bytes",
    )?;
    remote_beta(device, &["install", &options.app], true)?;
    let installed = physical_status(device, &options.app)?;
    ensure(
        installed.version.as_deref() == Some(entry.manifest().version())
            && installed.binary_sha256.as_deref()
                == Some(entry.manifest().binary_sha256().as_str()),
        "device did not activate the exact signed catalog binary",
    )?;

    let log_offset = physical_log_offset(device)?;
    *panel_session = Some(crate::panel::present_store_app(
        &options.app,
        device,
        PHYSICAL_LAUNCH_SECONDS,
    )?);
    std::thread::sleep(Duration::from_secs(3));
    physical_launch_health(device, &options.app)?;
    crate::shot_command(&[
        "--device".to_owned(),
        device.to_owned(),
        "--out".to_owned(),
        options.out.join("02-launched.png").display().to_string(),
    ])?;
    crate::panel::stop_store_app(
        panel_session
            .as_ref()
            .ok_or("acceptance panel session ownership was lost")?,
    )?;
    *panel_session = None;
    crate::shot_command(&[
        "--device".to_owned(),
        device.to_owned(),
        "--out".to_owned(),
        options
            .out
            .join("03-nickel-restored.png")
            .display()
            .to_string(),
    ])?;

    let target_before_remove = physical_target_digest(device, &options.app)?;
    remote_beta(device, &["uninstall", &options.app], true)?;
    ensure(
        !physical_status(device, &options.app)?.installed,
        "device removal was not committed",
    )?;
    remote_beta(device, &["install", &options.app], true)?;
    let after = physical_status(device, &options.app)?;
    ensure(
        after == installed,
        "reinstall did not restore the exact accepted bytes",
    )?;
    let mut preservation_after = physical_preservation(device, &options.app)?;
    preservation_after.target_state = physical_target_digest(device, &options.app)?;
    let mut preservation_before = preservation_before;
    preservation_before.target_state = target_before_remove;
    ensure(
        preservation_before == preservation_after,
        "unrelated apps, owner state, secrets, trust roots, data, or network ownership changed",
    )?;

    let logs = physical_logs(device, &options.app, log_offset)?;
    fs::write(options.out.join("logs.txt"), logs)
        .map_err(|error| format!("write redacted device logs: {error}"))?;
    let mut tampered_package = package.clone();
    tampered_package[0] ^= 1;
    ensure(
        parse_public_bundle(&tampered_package, &public).is_err(),
        "tampered package failure probe unexpectedly passed",
    )?;
    let mut tampered_signature = detached.as_bytes().to_owned();
    tampered_signature[0] ^= 1;
    ensure(
        verify(
            &catalog,
            &DetachedSignature::from_bytes(tampered_signature),
            &public,
        )
        .is_err(),
        "tampered catalog failure probe unexpectedly passed",
    )?;
    let marketing = capture_physical_marketing(
        &options.out,
        device,
        &options.app,
        &marketing_route,
        panel_width,
        panel_height,
        panel_session,
    )?;

    let mut evidence = Evidence {
        mode: "attended-device",
        app: options.app.clone(),
        catalog_url: catalog_url.to_owned(),
        catalog_sha256: crate::sha256::hex_digest(&catalog),
        catalog_signature: detached.to_hex(),
        public_key_fingerprint: crate::sha256::hex_digest(public.as_bytes()),
        package_sha256: crate::sha256::hex_digest(&package),
        package_bytes: package.len(),
        before,
        after,
        preservation_before,
        preservation_after,
        marketing,
        events: Vec::new(),
    };
    for (name, detail) in [
        ("device-identity", "profile, firmware, Cobalt and protocol matched the explicit confirmation"),
        ("clean-or-update", "device cached the archived catalog/signature bytes and activated their exact manifest version and binary SHA"),
        ("launch-success", "bounded panel session started and a screenshot was captured"),
        ("nickel-hand-back", "session stopped and the restored Nickel panel was captured"),
        ("remove-confirmation", "application was absent after the committed remove transaction"),
        ("reinstall", "the exact accepted version and binary SHA were restored"),
        ("owner-data-preservation", "protected before/after digests match exactly"),
        ("failure-probes", "tampered catalog signature and package bytes were rejected on the host"),
    ] {
        event(&mut evidence, name, detail);
    }
    evidence.events.push(Event {
        name: "marketing-capture".to_owned(),
        result: if evidence.marketing.complete {
            "passed"
        } else {
            "incomplete"
        }
        .to_owned(),
        detail: evidence.marketing.reason.as_ref().map_or_else(
            || {
                "post-acceptance public route captured title, interaction, and result without recording Nickel"
                    .to_owned()
            },
            |reason| format!("marketing artifacts are incomplete: {reason}"),
        ),
    });
    write_report(&options.out, &evidence)?;
    if !evidence.marketing.complete {
        return Err(
            "marketing capture is incomplete; see report.json and marketing/ffmpeg-command.sh"
                .to_owned(),
        );
    }
    println!(
        "attended Beta Store acceptance passed; evidence: {}",
        options.out.display()
    );
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the capture binds the accepted app, route, panel, evidence root, and owned session"
)]
fn capture_physical_marketing(
    evidence_root: &Path,
    device: &str,
    app: &str,
    route: &MarketingRoute,
    width: u32,
    height: u32,
    panel_session: &mut Option<crate::panel::StorePanelSession>,
) -> Result<MarketingEvidence, String> {
    prepare_marketing_interaction()?;
    let directory = evidence_root.join("marketing");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create marketing evidence directory: {error}"))?;
    write_marketing_route(&directory, route)?;
    let remote = format!("root@{device}");
    let _ignored = crate::run_remote_shell(
        &remote,
        &format!("rm -f '{}'\n", crate::RECORDING_ON_DEVICE),
        Duration::from_secs(30),
    );
    let route_millis = route.title_ms + route.result_ms;
    let seconds = route_millis.saturating_add(4_000).div_ceil(1_000).max(6);
    *panel_session = Some(crate::panel::present_store_app(
        app,
        device,
        seconds.saturating_add(60),
    )?);
    physical_launch_health(device, app)?;
    crate::panel::ensure_store_session_running(
        panel_session
            .as_ref()
            .ok_or("marketing panel session ownership was lost")?,
    )?;

    let record_arguments = [
        "--device".to_owned(),
        device.to_owned(),
        "--seconds".to_owned(),
        seconds.to_string(),
        "--fps".to_owned(),
        MARKETING_FPS.to_string(),
        "--out".to_owned(),
        directory.display().to_string(),
    ];
    let (capture_sender, capture_receiver) = std::sync::mpsc::channel();
    let (transfer_sender, transfer_receiver) = std::sync::mpsc::channel();
    let recorder = std::thread::spawn(move || {
        crate::record_command_notifying(
            &record_arguments,
            Some((capture_sender, transfer_receiver)),
        )
    });
    let route_result = (|| {
        wait_for_recording_start(device, Duration::from_secs(180))?;
        crate::panel::ensure_store_session_running(
            panel_session
                .as_ref()
                .ok_or("marketing panel session ownership was lost")?,
        )?;
        std::thread::sleep(Duration::from_millis(route.title_ms));
        crate::panel::ensure_store_session_running(
            panel_session
                .as_ref()
                .ok_or("marketing panel session ownership was lost")?,
        )?;
        perform_marketing_interaction(device, route.x, route.y)?;
        std::thread::sleep(Duration::from_millis(route.result_ms));
        crate::panel::ensure_store_session_running(
            panel_session
                .as_ref()
                .ok_or("marketing panel session ownership was lost")?,
        )?;
        Ok::<(), String>(())
    })();
    let capture_result = capture_receiver
        .recv_timeout(Duration::from_secs(seconds.saturating_add(60)))
        .map_err(|_| "the read-only marketing capture did not complete in time".to_owned());
    let session_result = capture_result.and_then(|()| {
        crate::panel::ensure_store_session_running(
            panel_session
                .as_ref()
                .ok_or("marketing panel session ownership was lost")?,
        )
    });
    let stop_result = crate::panel::stop_store_app(
        panel_session
            .as_ref()
            .ok_or("marketing panel session ownership was lost")?,
    );
    if stop_result.is_ok() {
        *panel_session = None;
    }
    let _ignored = transfer_sender.send(());
    let recording_result = recorder
        .join()
        .map_err(|_| "marketing recording thread panicked".to_owned())?;
    if let Err(error) = route_result
        .and(recording_result)
        .and(session_result)
        .and(stop_result)
    {
        let _ignored = fs::remove_dir_all(&directory);
        return Err(error);
    }

    match finalize_marketing(
        evidence_root,
        route,
        width,
        height,
        seconds.saturating_mul(1_000),
        discover_encoder_tools(),
    ) {
        Ok(evidence) => Ok(evidence),
        Err(error) => {
            let command_path = directory.join("ffmpeg-command.sh");
            if !command_path.exists() {
                fs::write(&command_path, marketing_command_text())
                    .map_err(|write_error| format!("{error}; write fallback: {write_error}"))?;
            }
            let artifacts = collect_marketing_artifacts(evidence_root, &directory)?;
            Ok(MarketingEvidence {
                complete: false,
                reason: Some(error),
                title: route.title.clone(),
                interaction: route.interaction.clone(),
                result: route.result.clone(),
                width,
                height,
                frame_count: artifacts
                    .iter()
                    .filter(|artifact| artifact.kind == "frame")
                    .count(),
                duration_ms: 0,
                fps: MARKETING_FPS,
                reproducible_command: relative_path(evidence_root, &command_path)?,
                artifacts,
            })
        }
    }
}

fn wait_for_recording_start(device: &str, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let remote = format!("root@{device}");
    while Instant::now() < deadline {
        let script = format!(
            "test -f '{}' && test \"$(wc -c < '{}')\" -gt 16\n",
            crate::RECORDING_ON_DEVICE,
            crate::RECORDING_ON_DEVICE
        );
        if crate::run_remote_shell(&remote, &script, Duration::from_secs(10))
            .is_ok_and(|output| output.status.success())
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err("timed out waiting for the read-only frame recorder".to_owned())
}

#[cfg(feature = "device-write")]
fn prepare_marketing_interaction() -> Result<(), String> {
    let mut build = crate::device_build_command("kobo-tap", Some("device-write"))?;
    crate::run_status(&mut build, "build fixed marketing interaction artifact")
}

#[cfg(not(feature = "device-write"))]
fn prepare_marketing_interaction() -> Result<(), String> {
    Err(
        "attended marketing capture requires a host CLI built with --features device-write"
            .to_owned(),
    )
}

#[cfg(feature = "device-write")]
fn perform_marketing_interaction(device: &str, x: u32, y: u32) -> Result<(), String> {
    crate::run_remote_fixed_artifact(device, &crate::RemoteArtifact::tap(format!("{x},{y}"), 0))
}

#[cfg(not(feature = "device-write"))]
fn perform_marketing_interaction(_device: &str, _x: u32, _y: u32) -> Result<(), String> {
    Err("marketing interaction support is not compiled in".to_owned())
}

fn remote_beta(device: &str, arguments: &[&str], write: bool) -> Result<String, String> {
    let quoted = arguments
        .iter()
        .map(|argument| format!("'{argument}'"))
        .collect::<Vec<_>>()
        .join(" ");
    let unlock = if write {
        format!("KOBO_BETA_STORE_UNLOCK={DEVICE_UNLOCK} ")
    } else {
        String::new()
    };
    let script = format!(
        "set -eu\nexec {unlock}'{}/bin/kobod' --beta-store {quoted}\n",
        crate::connect::INSTALL_DIRECTORY
    );
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        &script,
        Duration::from_secs(180),
    )?;
    if !output.status.success() {
        return Err(redact(&crate::remote_shell_error(
            format!("beta Store command exited with {}", output.status),
            &output.stdout,
            &output.stderr,
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn physical_status(device: &str, app: &str) -> Result<AppSnapshot, String> {
    let output = remote_beta(device, &["status", app], false)?;
    let values = fields(&output);
    let installed = values.get("installed").is_some_and(|value| value == "true");
    Ok(AppSnapshot {
        installed,
        version: installed.then(|| values.get("version").cloned()).flatten(),
        binary_sha256: installed
            .then(|| values.get("binary_sha256").cloned())
            .flatten(),
    })
}

fn physical_preservation(device: &str, app: &str) -> Result<Preservation, String> {
    let root = crate::connect::INSTALL_DIRECTORY;
    let script = format!(
        "set -eu\n\
         check_tree() {{\n\
           [ ! -e \"$1\" ] || ! find \"$1\" ! -type d ! -type f -print 2>/dev/null | grep -q . || {{\n\
             echo 'protected tree contains a non-regular filesystem object' >&2\n\
             exit 1\n\
           }}\n\
         }}\n\
         digest() {{\n\
           if [ ! -e \"$1\" ]; then printf absent; return; fi\n\
           if [ -d \"$1\" ]; then check_tree \"$1\"; elif [ ! -f \"$1\" ]; then exit 1; fi\n\
           find \"$1\" -type f -exec sha256sum {{}} \\; 2>/dev/null | sort | sha256sum | cut -d' ' -f1\n\
         }}\n\
         digest_without() {{\n\
           if [ ! -e \"$1\" ]; then printf absent; return; fi\n\
           check_tree \"$1\"\n\
           find \"$1\" -type f ! -path \"$1/$2/*\" -exec sha256sum {{}} \\; 2>/dev/null | \
             sort | sha256sum | cut -d' ' -f1\n\
         }}\n\
         {{\n\
           digest_without '{root}/state' '{app}'\n\
           digest '{root}/secrets'\n\
           digest '{root}/trust'\n\
           digest_without '{root}/data' '{app}'\n\
           check_tree '{root}/apps'\n\
           find '{root}/apps' -type f ! -path '{root}/apps/{app}/*' ! -path '{root}/apps/{app}.*/*' \
             -exec sha256sum {{}} \\; 2>/dev/null | sort | sha256sum | cut -d' ' -f1\n\
           digest '/mnt/onboard/.kobo/Kobo/Kobo eReader.conf'\n\
           digest '/etc/wpa_supplicant/wpa_supplicant.conf'\n\
         }} | sha256sum | sed 's/ .*//;s/^/unrelated=/'\n\
         digest_without '{root}/state' '{app}' | sed 's/^/state=/'\n\
         {{ digest '{root}/state/{app}'; digest '{root}/data/{app}'; }} | \
           sha256sum | sed 's/ .*//;s/^/target_state=/'\n"
    );
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        &script,
        Duration::from_secs(120),
    )?;
    if !output.status.success() {
        return Err("could not compute owner-data preservation digests".to_owned());
    }
    let values = fields(&String::from_utf8_lossy(&output.stdout));
    Ok(Preservation {
        unrelated: values.get("unrelated").cloned().unwrap_or_default(),
        state: values.get("state").cloned().unwrap_or_default(),
        target_state: values.get("target_state").cloned().unwrap_or_default(),
    })
}

fn physical_target_digest(device: &str, app: &str) -> Result<String, String> {
    let root = crate::connect::INSTALL_DIRECTORY;
    let script = format!(
        "check_tree() {{\n\
           [ ! -e \"$1\" ] || ! find \"$1\" ! -type d ! -type f -print 2>/dev/null | grep -q . || exit 1\n\
         }}\n\
         digest() {{\n\
           if [ ! -e \"$1\" ]; then printf absent; return; fi\n\
           check_tree \"$1\"\n\
           find \"$1\" -type f -exec sha256sum {{}} \\; 2>/dev/null | sort | sha256sum | cut -d' ' -f1\n\
         }}\n\
         {{ digest '{root}/state/{app}'; digest '{root}/data/{app}'; }} | \
           sha256sum | cut -d' ' -f1\n"
    );
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        &script,
        Duration::from_secs(120),
    )?;
    if !output.status.success() {
        return Err("could not compute target state/data digest".to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn ensure_no_panel_session(device: &str) -> Result<(), String> {
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        "if pidof kobod >/dev/null 2>&1; then \
           echo 'another Cobalt panel session is already running' >&2; exit 1; \
         fi\n",
        Duration::from_secs(30),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err("another Cobalt panel session is already running; exit it before acceptance".to_owned())
    }
}

fn physical_log_offset(device: &str) -> Result<u64, String> {
    let script = "if [ -f /mnt/onboard/.kobo-blackbox.log ]; then \
                    wc -c < /mnt/onboard/.kobo-blackbox.log; \
                  else printf '0\\n'; fi\n";
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        script,
        Duration::from_secs(30),
    )?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|_| "device returned an invalid blackbox offset".to_owned())
}

fn physical_logs(device: &str, app: &str, offset: u64) -> Result<String, String> {
    let script = format!(
        "if [ -f /mnt/onboard/.kobo-blackbox.log ]; then \
           dd if=/mnt/onboard/.kobo-blackbox.log bs=1 skip={offset} 2>/dev/null | \
           grep -E 'session|reader|launch|application|{app}' || true; \
         fi\n"
    );
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        &script,
        Duration::from_secs(30),
    )?;
    Ok(redact(&String::from_utf8_lossy(&output.stdout)))
}

fn physical_launch_health(device: &str, app: &str) -> Result<(), String> {
    let log = format!("/tmp/kobo-{app}.log");
    let script = format!(
        "set -eu\n\
         pidof kobod >/dev/null\n\
         if [ -f '{log}' ] && grep -Eqi 'identity mismatch|failed to greet|panicked|protocol.*(invalid|mismatch)' '{log}'; then\n\
           tail -n 20 '{log}' >&2\n\
           exit 1\n\
         fi\n"
    );
    let output = crate::panel::run_remote_shell_waking(
        &format!("root@{device}"),
        &script,
        Duration::from_secs(30),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(redact(
            "the application failed its opening protocol exchange",
        ))
    }
}

fn fields(output: &str) -> BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

fn redact(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            if looks_like_ipv4(word)
                || looks_like_mac(word)
                || word.to_ascii_lowercase().contains("token=")
                || word.to_ascii_lowercase().contains("authorization:")
                || word.to_ascii_lowercase().contains("password=")
                || word.to_ascii_lowercase().contains("ssid=")
            {
                "[redacted]".to_owned()
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_ipv4(value: &str) -> bool {
    let trimmed =
        value.trim_matches(|character: char| !character.is_ascii_digit() && character != '.');
    let parts = trimmed.split('.').collect::<Vec<_>>();
    parts.len() == 4 && parts.iter().all(|part| part.parse::<u8>().is_ok())
}

fn looks_like_mac(value: &str) -> bool {
    let trimmed =
        value.trim_matches(|character: char| !character.is_ascii_hexdigit() && character != ':');
    let parts = trimmed.split(':').collect::<Vec<_>>();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let path = PathBuf::from("target/beta-store-smoke-tests").join(name);
        let _ignored = fs::remove_dir_all(&path);
        path
    }

    fn fixture(path: &Path) {
        fs::create_dir_all(path).expect("fixture directory");
        fs::write(path.join(FIXTURE_SEED), format!("{}\n", "2a".repeat(32))).expect("fixture seed");
        fs::write(path.join(FIXTURE_MARKETING_ROUTE), route_text()).expect("marketing route");
    }

    fn route_text() -> &'static str {
        "cobalt-beta-marketing-route 1\n\
         privacy public-demo-no-owner-data\n\
         title Beta acceptance\n\
         interaction Open result|64|48\n\
         result Accepted result\n\
         timing 1200|2200\n"
    }

    fn route(path: &Path) -> MarketingRoute {
        fs::create_dir_all(path.parent().expect("route parent")).expect("route parent");
        fs::write(path, route_text()).expect("route");
        read_marketing_route(path).expect("parsed route")
    }

    #[cfg(unix)]
    fn fake_encoders(root: &Path) -> EncoderTools {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(root).expect("encoder directory");
        let ffmpeg = root.join("ffmpeg");
        fs::write(
            &ffmpeg,
            "#!/bin/sh\nfor last do :; done\ncp frame-0000.png \"$last\"\n",
        )
        .expect("fake ffmpeg");
        let ffprobe = root.join("ffprobe");
        fs::write(
            &ffprobe,
            "#!/bin/sh\nprintf 'width=320\\nheight=240\\nduration=4.400000\\n'\n",
        )
        .expect("fake ffprobe");
        for path in [&ffmpeg, &ffprobe] {
            let mut permissions = fs::metadata(path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("executable");
        }
        EncoderTools {
            ffmpeg: ffmpeg.canonicalize().expect("ffmpeg path"),
            ffprobe: ffprobe.canonicalize().expect("ffprobe path"),
        }
    }

    #[test]
    fn parser_refuses_stable_or_underconfirmed_device_runs() {
        let out = root("parser");
        let base = [
            "--app",
            "fixture",
            "--beta-catalog",
            kobod::app_store::CATALOG_URL,
            "--device",
            "192.0.2.2",
            "--out",
            out.to_str().expect("path"),
            "--expected-profile",
            "CLARA_BW_391",
            "--expected-cobalt",
            "0.3.4",
            "--expected-firmware",
            "4.45.23697",
            "--marketing-route",
            "routes/public.txt",
            "--confirm",
            "CLARA_BW_391/Cobalt-0.3.4/4.45.23697",
        ]
        .map(str::to_owned);
        assert!(parse(&base).is_err());

        let mut beta = base;
        beta[3] = kobod::app_store::BETA_CATALOG_URL.to_owned();
        beta[beta.len() - 1] = "wrong".to_owned();
        assert!(parse(&beta).is_err());
        beta[beta.len() - 1] = "CLARA_BW_391/Cobalt-0.3.4/4.45.23697".to_owned();
        assert!(parse(&beta).is_ok());
    }

    #[test]
    fn mock_command_runs_the_complete_acceptance_matrix() {
        let fixture_path = root("fixture");
        fixture(&fixture_path);
        let out = root("evidence");
        let encoders_available = discover_encoder_tools().is_ok();
        let result = command(
            &[
                "--app",
                "fixture",
                "--fixture",
                fixture_path.to_str().expect("fixture path"),
                "--out",
                out.to_str().expect("output path"),
            ]
            .map(str::to_owned),
        );
        assert_eq!(result.is_ok(), encoders_available, "{result:?}");
        let report = fs::read_to_string(out.join("report.json")).expect("report");
        for scenario in [
            "clean-install",
            "already-installed-rerun",
            "update",
            "downgrade-refusal",
            "remove-confirmation",
            "reinstall",
            "interrupted-download",
            "checksum-failure",
            "signature-failure",
            "malformed-catalog",
            "malformed-package",
            "command-conflict",
            "atomic-rollback",
            "owner-data-preservation",
            "launch-failure",
        ] {
            assert!(report.contains(scenario), "missing {scenario}");
        }
        assert!(out.join("01-clean-install.png").is_file());
        assert!(out.join("02-recovered-launch.png").is_file());
        assert!(out.join("marketing/frame-0000.png").is_file());
        assert!(out.join("marketing/ffmpeg-command.sh").is_file());
        if encoders_available {
            assert!(out.join("marketing/marketing.mp4").is_file());
            assert!(out.join("marketing/marketing.webm").is_file());
            assert!(out.join("marketing/marketing.gif").is_file());
            assert!(report.contains("\"marketing\":{\"complete\":true"));
        } else {
            assert!(report.contains("\"marketing\":{\"complete\":false"));
        }
    }

    #[test]
    fn missing_encoders_retain_frames_and_reproducible_command() {
        let out = root("marketing-missing");
        let route = route(&out.join("route.txt"));
        let evidence = create_mock_marketing(
            &out,
            &route,
            Err("ffmpeg is not available on PATH".to_owned()),
        )
        .expect("incomplete marketing evidence");
        assert!(!evidence.complete);
        assert_eq!(evidence.frame_count, 3);
        assert!(out.join("marketing/frame-0000.png").is_file());
        assert!(out.join("marketing/ffmpeg-command.sh").is_file());
        assert!(evidence
            .artifacts
            .iter()
            .all(|artifact| artifact.sha256.len() == 64));
        for artifact in &evidence.artifacts {
            let bytes = fs::read(out.join(&artifact.path)).expect("hashed artifact");
            assert_eq!(artifact.sha256, crate::sha256::hex_digest(&bytes));
        }
        fs::write(out.join("marketing/recording.mp4"), b"unvalidated").expect("generic video");
        let evidence = finalize_marketing(
            &out,
            &route,
            320,
            240,
            4_400,
            Err("ffprobe is not available on PATH".to_owned()),
        )
        .expect("missing validator evidence");
        assert!(!out.join("marketing/recording.mp4").exists());
        assert!(!evidence
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "mp4"));
    }

    #[cfg(unix)]
    #[test]
    fn available_encoders_produce_validated_hashed_marketing_artifacts() {
        let out = root("marketing-encoded");
        let route = route(&out.join("route.txt"));
        let tools = fake_encoders(&out.join("encoders"));
        let evidence =
            create_mock_marketing(&out, &route, Ok(tools)).expect("complete marketing evidence");
        assert!(evidence.complete, "{:?}", evidence.reason);
        for kind in ["mp4", "webm", "gif"] {
            assert!(evidence
                .artifacts
                .iter()
                .any(|artifact| artifact.kind == kind && artifact.bytes > 0));
        }
        for artifact in &evidence.artifacts {
            let bytes = fs::read(out.join(&artifact.path)).expect("hashed artifact");
            assert_eq!(artifact.sha256, crate::sha256::hex_digest(&bytes));
        }
    }

    #[test]
    fn frame_continuity_and_public_route_labels_fail_closed() {
        let out = root("marketing-invalid");
        let route_path = out.join("route.txt");
        fs::create_dir_all(&out).expect("invalid route root");
        fs::write(
            &route_path,
            route_text().replace("Beta acceptance", "ssid=Private"),
        )
        .expect("invalid route");
        assert!(read_marketing_route(&route_path).is_err());

        let route = route(&route_path);
        create_mock_marketing(
            &out,
            &route,
            Err("ffmpeg is not available on PATH".to_owned()),
        )
        .expect("frames");
        fs::rename(
            out.join("marketing/frame-0001.png"),
            out.join("marketing/frame-0004.png"),
        )
        .expect("break continuity");
        assert!(validate_marketing_frames(&out.join("marketing"), 320, 240).is_err());
    }

    #[test]
    fn marketing_frames_must_cover_title_interaction_and_result_milestones() {
        let route = MarketingRoute {
            title: "Title".to_owned(),
            interaction: "Open".to_owned(),
            result: "Result".to_owned(),
            x: 10,
            y: 10,
            title_ms: 1_200,
            result_ms: 2_200,
        };
        let early = vec![
            ("frame-0000.png".to_owned(), 0),
            ("frame-0001.png".to_owned(), 100),
            ("frame-0002.png".to_owned(), 200),
        ];
        assert!(validate_marketing_milestones(&early, &route).is_err());
        let complete = vec![
            ("frame-0000.png".to_owned(), 0),
            ("frame-0001.png".to_owned(), 1_200),
            ("frame-0002.png".to_owned(), 1_500),
        ];
        assert!(validate_marketing_milestones(&complete, &route).is_ok());
    }

    #[test]
    fn logs_redact_network_and_credential_identifiers() {
        let redacted =
            redact("peer 192.168.1.9 aa:bb:cc:dd:ee:ff token=abc password=def ssid=Home ok");
        assert!(!redacted.contains("192.168.1.9"));
        assert!(!redacted.contains("aa:bb:cc:dd:ee:ff"));
        assert!(!redacted.contains("Home"));
        assert!(redacted.ends_with("ok"));
    }

    #[test]
    fn target_state_changes_are_separate_from_unrelated_preservation() {
        let root = root("preservation");
        seed_owner_state(&root, "fixture").expect("owner state");
        let before = preservation(&root, "fixture").expect("before");
        let target_data = root.join("data/fixture/cache");
        fs::create_dir_all(target_data.parent().expect("target data parent"))
            .expect("target data directory");
        fs::write(target_data, b"legitimate launch state").expect("target data");
        let after = preservation(&root, "fixture").expect("after");
        assert_eq!(before.unrelated, after.unrelated);
        assert_eq!(before.state, after.state);
        assert_ne!(before.target_state, after.target_state);
    }
}
