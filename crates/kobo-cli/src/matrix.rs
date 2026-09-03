//! Deterministic profile/app validation without a browser or physical reader.

use super::{
    wait_for_app, workspace_host_binary, AppChild, DevSessionGuard, INSTALLED_PACKAGES,
    STORE_PACKAGES,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const DRIVE_ROUTE: &str = include_str!("../../../apps/backgammon/drive.txt");
const PROBE_PICTURE_SIDE: u32 = 256;
const PROBE_PICTURES: u32 = 5;

#[derive(Debug)]
struct Failure {
    kind: String,
    profile: String,
    rotation: u32,
    subject: String,
    error: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Screenshot {
    path: String,
    width: u32,
    height: u32,
    sha256: String,
}

#[derive(Clone, Copy, Default)]
struct Counts {
    initial: usize,
    scenarios: usize,
    drives: usize,
    warnings: usize,
}

struct Options {
    report: PathBuf,
    screenshots: PathBuf,
    skip_build: bool,
    worker: Option<(String, u32)>,
}

struct ScenarioProbe {
    scenario: kobo_sim::Scenario,
    result: &'static str,
    picture: Option<kobo_sdk::TilePicture>,
}

impl ScenarioProbe {
    fn new(scenario: kobo_sim::Scenario) -> Self {
        Self {
            scenario,
            result: "pending",
            picture: None,
        }
    }

    fn show(&self, context: &mut kobo_sdk::Context) {
        let mut screen = kobo_sdk::ScreenBuilder::new("matrix-scenario")
            .heading("Simulator scenario")
            .text(format!("scenario:{}", self.scenario.name()))
            .text(format!("result:{}", self.result));
        if let Some(picture) = self.picture {
            screen = screen.picture(picture, 25);
        }
        context.set_screen(screen.build());
    }

    fn set_result(&mut self, context: &mut kobo_sdk::Context, result: &'static str) {
        self.result = result;
        self.show(context);
    }
}

impl kobo_sdk::KoboApp for ScenarioProbe {
    fn on_start(&mut self, context: &mut kobo_sdk::Context) {
        match self.scenario {
            kobo_sim::Scenario::Normal | kobo_sim::Scenario::LowBattery => {
                context.device().read_battery();
            }
            kobo_sim::Scenario::Offline
            | kobo_sim::Scenario::HostDown
            | kobo_sim::Scenario::NetworkTimeout => {
                let _ = context.spawn(kobo_sdk::Task::Fetch {
                    url: "https://example.invalid/matrix".to_owned(),
                    offset: 0,
                    max_bytes: 16,
                    credential: None,
                    headers: Vec::new(),
                });
            }
            kobo_sim::Scenario::PermissionDenied => {
                context.applications().cached_catalog();
            }
            kobo_sim::Scenario::MissingSecret => {
                let _ = context.spawn(kobo_sdk::Task::Post {
                    url: "https://example.invalid/matrix".to_owned(),
                    body: "{}".to_owned(),
                    content_type: "application/json".to_owned(),
                    credential: Some(kobo_sdk::Credential::bearer("matrix-missing")),
                    headers: Vec::new(),
                    max_bytes: 16,
                });
            }
            kobo_sim::Scenario::StorageFull => {
                context.store().save("matrix-probe", b"saved".to_vec());
            }
            kobo_sim::Scenario::CachePressure => {
                for handle in 1..=PROBE_PICTURES {
                    let picture = context.put_picture(
                        kobo_sdk::PictureHandle(handle),
                        PROBE_PICTURE_SIDE,
                        PROBE_PICTURE_SIDE,
                        vec![
                            u8::try_from(handle).unwrap_or(u8::MAX);
                            (PROBE_PICTURE_SIDE * PROBE_PICTURE_SIDE) as usize
                        ],
                    );
                    if handle == 1 {
                        self.picture = picture;
                    }
                }
                self.result = "picture-evicted";
            }
        }
        self.show(context);
    }

    fn on_action(&mut self, _context: &mut kobo_sdk::Context, _action: kobo_sdk::ActionId) {}

    fn on_device_result(
        &mut self,
        context: &mut kobo_sdk::Context,
        _request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        let result = match (self.scenario, result) {
            (
                kobo_sim::Scenario::Normal,
                kobo_sdk::DeviceResult::Battery {
                    percent: 72,
                    charging: false,
                },
            ) => "battery-72",
            (
                kobo_sim::Scenario::LowBattery,
                kobo_sdk::DeviceResult::Battery {
                    percent: 5,
                    charging: false,
                },
            ) => "battery-5",
            (
                kobo_sim::Scenario::PermissionDenied,
                kobo_sdk::DeviceResult::Denied(kobo_sdk::DenyReason::NotDeclared),
            ) => "denied-not-declared",
            _ => "unexpected-device-result",
        };
        self.set_result(context, result);
    }

    fn on_task(
        &mut self,
        context: &mut kobo_sdk::Context,
        _task: kobo_sdk::TaskId,
        outcome: kobo_sdk::TaskOutcome,
    ) {
        let result = match (self.scenario, outcome) {
            (
                kobo_sim::Scenario::Offline,
                kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::Offline),
            ) => "offline",
            (
                kobo_sim::Scenario::HostDown,
                kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
            ) => "unreachable",
            (
                kobo_sim::Scenario::MissingSecret,
                kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::NotFound),
            ) => "missing-secret",
            (
                kobo_sim::Scenario::NetworkTimeout,
                kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::TimedOut),
            ) => "timed-out",
            _ => "unexpected-task-result",
        };
        self.set_result(context, result);
    }

    fn on_store(&mut self, context: &mut kobo_sdk::Context, result: kobo_sdk::StoreResult) {
        let result = match (self.scenario, result) {
            (
                kobo_sim::Scenario::StorageFull,
                kobo_sdk::StoreResult::Denied(kobo_sdk::StoreError::TooFull),
            ) => "storage-too-full",
            _ => "unexpected-store-result",
        };
        self.set_result(context, result);
    }
}

pub fn run_probe(arguments: &[String]) -> Result<(), String> {
    let [scenario] = arguments else {
        return Err("internal matrix probe needs one scenario".to_owned());
    };
    let scenario = kobo_sim::Scenario::parse(scenario.as_bytes())
        .ok_or_else(|| format!("unknown internal matrix scenario {scenario:?}"))?;
    kobo_sdk::run("store", ScenarioProbe::new(scenario))
        .map_err(|error| format!("matrix scenario probe: {error}"))
}

pub fn run(arguments: &[String]) -> Result<(), String> {
    if matches!(arguments, [help] if matches!(help.as_str(), "--help" | "-h")) {
        println!("{USAGE}");
        return Ok(());
    }
    let options = parse(arguments)?;
    validate_output_paths(&options.report, &options.screenshots)?;
    let packages = packages();
    if !options.skip_build {
        build(&packages)?;
    }
    if options.worker.is_none() {
        prepare_screenshot_directory(&options.screenshots)?;
    } else {
        fs::create_dir_all(&options.screenshots)
            .map_err(|error| format!("create {}: {error}", options.screenshots.display()))?;
    }
    if let Some(parent) = options.report.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }

    if let Some((profile, rotation)) = options.worker {
        let config = kobo_sim::SimulationConfig::select(&profile, Some(rotation))?;
        return execute(&packages, &[config], &options.report, &options.screenshots);
    }
    coordinate(&packages, &options.report, &options.screenshots)
}

fn coordinate(packages: &[&str], report: &Path, screenshots: &Path) -> Result<(), String> {
    let configs = kobo_sim::SimulationConfig::supported().collect::<Vec<_>>();
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    let mut screenshots_inventory = Vec::new();
    let mut coverage = BTreeMap::<(&'static str, u32), Counts>::new();
    let executable =
        std::env::current_exe().map_err(|error| format!("locate matrix executable: {error}"))?;

    for config in configs.iter().copied() {
        let worker_report = std::env::temp_dir().join(format!(
            "km-{}-{}-{}.json",
            std::process::id(),
            config.profile().id,
            config.pose().rotation()
        ));
        let output = Command::new(&executable)
            .args([
                "matrix",
                "--report",
                worker_report
                    .to_str()
                    .ok_or("worker report path is not UTF-8")?,
                "--screenshots",
                screenshots.to_str().ok_or("screenshot path is not UTF-8")?,
                "--skip-build",
                "--worker-profile",
                config.profile().id,
                "--worker-rotation",
                &config.pose().rotation().to_string(),
            ])
            .output()
            .map_err(|error| format!("run {} matrix worker: {error}", config.profile().id))?;
        let parsed = read_worker_report(&worker_report)?;
        let _ = fs::remove_file(&worker_report);
        if !output.status.success() && parsed.1.is_empty() {
            return Err(format!(
                "{} rotation {} worker exited with {}\n{}",
                config.profile().id,
                config.pose().rotation(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        add_counts(&mut counts, parsed.0);
        coverage.insert((config.profile().id, config.pose().rotation()), parsed.0);
        failures.extend(parsed.1);
        screenshots_inventory.extend(parsed.2);
    }
    verify_screenshot_inventory(screenshots, &screenshots_inventory)?;

    finish_report(
        packages,
        &configs,
        report,
        &counts,
        &coverage,
        &failures,
        &screenshots_inventory,
    )
}

fn execute(
    packages: &[&str],
    configs: &[kobo_sim::SimulationConfig],
    report: &Path,
    screenshots: &Path,
) -> Result<(), String> {
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    let mut screenshots_inventory = Vec::new();
    let mut coverage = BTreeMap::<(&'static str, u32), Counts>::new();

    for config in configs.iter().copied() {
        for package in packages {
            counts.initial += 1;
            coverage
                .entry((config.profile().id, config.pose().rotation()))
                .or_default()
                .initial += 1;
            match initial_case(package, config) {
                Ok((warnings, frame)) => {
                    counts.warnings += warnings;
                    coverage
                        .entry((config.profile().id, config.pose().rotation()))
                        .or_default()
                        .warnings += warnings;
                    if *package == "kobo-store" {
                        screenshots_inventory.push(write_shot(
                            screenshots,
                            config,
                            "store-initial",
                            &frame,
                        )?);
                    }
                }
                Err(error) => failures.push(Failure {
                    kind: "initial".to_owned(),
                    profile: config.profile().id.to_owned(),
                    rotation: config.pose().rotation(),
                    subject: (*package).to_owned(),
                    error,
                }),
            }
        }

        for scenario in kobo_sim::Scenario::ALL {
            counts.scenarios += 1;
            coverage
                .entry((config.profile().id, config.pose().rotation()))
                .or_default()
                .scenarios += 1;
            if let Err(error) = scenario_case(config.with_scenario(scenario)) {
                failures.push(Failure {
                    kind: "scenario".to_owned(),
                    profile: config.profile().id.to_owned(),
                    rotation: config.pose().rotation(),
                    subject: scenario.name().to_owned(),
                    error,
                });
            }
        }

        counts.drives += 1;
        coverage
            .entry((config.profile().id, config.pose().rotation()))
            .or_default()
            .drives += 1;
        match drive_case(config, screenshots) {
            Ok(shots) => screenshots_inventory.extend(shots),
            Err(error) => {
                failures.push(Failure {
                    kind: "drive".to_owned(),
                    profile: config.profile().id.to_owned(),
                    rotation: config.pose().rotation(),
                    subject: "apps/backgammon/drive.txt".to_owned(),
                    error,
                });
            }
        }
    }

    finish_report(
        packages,
        configs,
        report,
        &counts,
        &coverage,
        &failures,
        &screenshots_inventory,
    )
}

fn finish_report(
    packages: &[&str],
    configs: &[kobo_sim::SimulationConfig],
    report: &Path,
    counts: &Counts,
    coverage: &BTreeMap<(&'static str, u32), Counts>,
    failures: &[Failure],
    screenshots: &[Screenshot],
) -> Result<(), String> {
    let json = report_json(packages, coverage, counts, failures, screenshots);
    fs::write(report, json).map_err(|error| format!("write {}: {error}", report.display()))?;
    println!(
        "matrix: {} profiles, {} poses, {} apps, {} cases, {} failures; report {}",
        kobo_profile::SUPPORTED_PROFILES.len(),
        configs.len(),
        packages.len(),
        counts.initial + counts.scenarios + counts.drives,
        failures.len(),
        report.display()
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} matrix case(s) failed; see {}",
            failures.len(),
            report.display()
        ))
    }
}

fn add_counts(total: &mut Counts, added: Counts) {
    total.initial += added.initial;
    total.scenarios += added.scenarios;
    total.drives += added.drives;
    total.warnings += added.warnings;
}

fn read_worker_report(path: &Path) -> Result<(Counts, Vec<Failure>, Vec<Screenshot>), String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let root =
        kobo_json::parse(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
    let counts = root
        .get("counts")
        .ok_or_else(|| format!("{} has no counts", path.display()))?;
    let number = |name| {
        let kobo_json::Value::Number(value) = counts
            .get(name)
            .ok_or_else(|| format!("{} has no count {name}", path.display()))?
        else {
            return Err(format!("{} count {name} is not numeric", path.display()));
        };
        if *value < 0.0 || value.fract() != 0.0 {
            return Err(format!("{} count {name} is not an integer", path.display()));
        }
        value
            .to_string()
            .parse::<usize>()
            .map_err(|_| format!("{} count {name} is out of range", path.display()))
    };
    let counts = Counts {
        initial: number("initial")?,
        scenarios: number("scenarios")?,
        drives: number("drives")?,
        warnings: number("warnings")?,
    };
    let failures = root
        .get("failures")
        .and_then(kobo_json::Value::as_array)
        .ok_or_else(|| format!("{} has no failures array", path.display()))?
        .iter()
        .map(|failure| {
            let string = |name| {
                failure
                    .get(name)
                    .and_then(kobo_json::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("worker failure has no string {name}"))
            };
            let rotation = match failure.get("rotation") {
                Some(kobo_json::Value::Number(value)) if *value >= 0.0 && value.fract() == 0.0 => {
                    value
                        .to_string()
                        .parse::<u32>()
                        .map_err(|_| "worker failure rotation is out of range".to_owned())?
                }
                _ => return Err("worker failure has no integer rotation".to_owned()),
            };
            Ok(Failure {
                kind: string("kind")?,
                profile: string("profile")?,
                rotation,
                subject: string("subject")?,
                error: string("error")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let screenshots = root
        .get("screenshots")
        .and_then(kobo_json::Value::as_array)
        .ok_or_else(|| format!("{} has no screenshots array", path.display()))?
        .iter()
        .map(read_worker_screenshot)
        .collect::<Result<Vec<_>, String>>()?;
    Ok((counts, failures, screenshots))
}

fn read_worker_screenshot(screenshot: &kobo_json::Value) -> Result<Screenshot, String> {
    let string = |name| {
        screenshot
            .get(name)
            .and_then(kobo_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("worker screenshot has no string {name}"))
    };
    let number = |name| {
        let Some(kobo_json::Value::Number(value)) = screenshot.get(name) else {
            return Err(format!("worker screenshot has no numeric {name}"));
        };
        if *value < 0.0 || value.fract() != 0.0 {
            return Err(format!("worker screenshot {name} is not an integer"));
        }
        value
            .to_string()
            .parse::<u32>()
            .map_err(|_| format!("worker screenshot {name} is out of range"))
    };
    let sha256 = string("sha256")?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("worker screenshot has an invalid SHA-256".to_owned());
    }
    Ok(Screenshot {
        path: string("path")?,
        width: number("width")?,
        height: number("height")?,
        sha256,
    })
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut report = None;
    let mut screenshots = None;
    let mut skip_build = false;
    let mut worker_profile = None;
    let mut worker_rotation = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--report" => {
                report = Some(PathBuf::from(
                    arguments.get(index + 1).ok_or("--report needs a path")?,
                ));
                index += 1;
            }
            "--screenshots" => {
                screenshots = Some(PathBuf::from(
                    arguments
                        .get(index + 1)
                        .ok_or("--screenshots needs a directory")?,
                ));
                index += 1;
            }
            "--skip-build" => skip_build = true,
            "--worker-profile" => {
                worker_profile = Some(
                    arguments
                        .get(index + 1)
                        .ok_or("--worker-profile needs an id")?
                        .clone(),
                );
                index += 1;
            }
            "--worker-rotation" => {
                worker_rotation = Some(
                    arguments
                        .get(index + 1)
                        .ok_or("--worker-rotation needs a number")?
                        .parse::<u32>()
                        .map_err(|_| "--worker-rotation needs a number")?,
                );
                index += 1;
            }
            other => return Err(format!("unknown matrix option {other:?}\n{USAGE}")),
        }
        index += 1;
    }
    let worker = match (worker_profile, worker_rotation) {
        (None, None) => None,
        (Some(profile), Some(rotation)) => Some((profile, rotation)),
        _ => return Err("matrix worker profile and rotation must be provided together".to_owned()),
    };
    Ok(Options {
        report: report.ok_or_else(|| USAGE.to_owned())?,
        screenshots: screenshots.ok_or_else(|| USAGE.to_owned())?,
        skip_build,
        worker,
    })
}

const USAGE: &str = "usage: kobo matrix --report PATH --screenshots DIR [--skip-build]";

fn validate_output_paths(report: &Path, screenshots: &Path) -> Result<(), String> {
    let report = resolved_path(report)?;
    let screenshots = resolved_path(screenshots)?;
    if report.starts_with(&screenshots) {
        return Err(format!(
            "matrix report {} must be outside screenshot directory {}",
            report.display(),
            screenshots.display()
        ));
    }
    Ok(())
}

fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("locate current directory: {error}"))?
            .join(path)
    };
    reject_dangling_symlinks(&absolute)?;
    for existing in absolute.ancestors() {
        match fs::canonicalize(existing) {
            Ok(canonical) => {
                let missing = absolute
                    .strip_prefix(existing)
                    .map_err(|error| format!("resolve output path {}: {error}", path.display()))?;
                return Ok(normalize_path(&canonical.join(missing)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("resolve output path {}: {error}", path.display())),
        }
    }
    Err(format!("resolve output path {}", path.display()))
}

fn reject_dangling_symlinks(path: &Path) -> Result<(), String> {
    for component in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(component) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                match fs::canonicalize(component) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(format!(
                            "output path {} contains dangling symlink {}",
                            path.display(),
                            component.display()
                        ));
                    }
                    Err(error) => {
                        return Err(format!(
                            "resolve output symlink {}: {error}",
                            component.display()
                        ));
                    }
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "inspect output path component {}: {error}",
                    component.display()
                ));
            }
        }
    }
    Ok(())
}

fn normalize_path(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut path, component| {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    path.pop();
                }
                component => path.push(component.as_os_str()),
            }
            path
        })
}

fn prepare_screenshot_directory(path: &Path) -> Result<(), String> {
    match fs::read_dir(path) {
        Ok(mut entries) => {
            if entries
                .next()
                .transpose()
                .map_err(|error| {
                    format!("inspect screenshot directory {}: {error}", path.display())
                })?
                .is_some()
            {
                return Err(format!(
                    "screenshot directory {} must be absent or empty",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| format!("create {}: {error}", path.display()))
        }
        Err(error) => Err(format!(
            "inspect screenshot directory {}: {error}",
            path.display()
        )),
    }
}

fn verify_screenshot_inventory(directory: &Path, inventory: &[Screenshot]) -> Result<(), String> {
    let reported = inventory
        .iter()
        .map(|screenshot| screenshot.path.as_str())
        .collect::<BTreeSet<_>>();
    if reported.len() != inventory.len() {
        return Err("matrix screenshot inventory contains duplicate paths".to_owned());
    }
    let actual = fs::read_dir(directory)
        .map_err(|error| format!("read screenshot directory {}: {error}", directory.display()))?
        .map(|entry| {
            let entry = entry.map_err(|error| {
                format!("read screenshot entry in {}: {error}", directory.display())
            })?;
            if !entry
                .file_type()
                .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?
                .is_file()
            {
                return Err(format!(
                    "unexpected non-file screenshot artifact {}",
                    entry.path().display()
                ));
            }
            Ok(entry.path().to_string_lossy().into_owned())
        })
        .collect::<Result<BTreeSet<_>, String>>()?;
    if actual != reported.iter().map(|path| (*path).to_owned()).collect() {
        return Err("matrix screenshot directory and JSON inventory differ".to_owned());
    }
    for screenshot in inventory {
        let bytes = fs::read(&screenshot.path)
            .map_err(|error| format!("read screenshot {}: {error}", screenshot.path))?;
        let hash = ring::digest::digest(&ring::digest::SHA256, &bytes);
        let actual_hash = hex_digest(hash.as_ref());
        if actual_hash != screenshot.sha256 {
            return Err(format!("screenshot hash changed for {}", screenshot.path));
        }
        let dimensions = png_dimensions(&bytes)
            .ok_or_else(|| format!("screenshot is not a PNG: {}", screenshot.path))?;
        if dimensions != (screenshot.width, screenshot.height) {
            return Err(format!(
                "screenshot dimensions for {} are {}x{}, expected {}x{}",
                screenshot.path, dimensions.0, dimensions.1, screenshot.width, screenshot.height
            ));
        }
    }
    Ok(())
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..16)? != b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR" {
        return None;
    }
    Some((
        u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
        u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
    ))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut encoded, byte| {
            let _ = write!(encoded, "{byte:02x}");
            encoded
        },
    )
}

fn packages() -> Vec<&'static str> {
    INSTALLED_PACKAGES
        .iter()
        .map(|(package, _)| *package)
        .filter(|package| *package != "kobod")
        .chain(STORE_PACKAGES.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn build(packages: &[&str]) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command.arg("build").arg("--quiet");
    for package in packages {
        command.arg("-p").arg(package);
    }
    let status = command
        .status()
        .map_err(|error| format!("build matrix applications: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("matrix application build exited with {status}"))
    }
}

fn initial_case(
    package: &str,
    config: kobo_sim::SimulationConfig,
) -> Result<(usize, Vec<u8>), String> {
    let (session, app) = start_app(package, config, true)?;
    let result = validate_session(&session);
    let _ = session.close();
    drop(app);
    result
}

fn start_app(
    package: &str,
    config: kobo_sim::SimulationConfig,
    block_network: bool,
) -> Result<(kobo_sim::AppSession, AppChild), String> {
    let command = Command::new(workspace_host_binary(package));
    start_process(command, config, block_network)
}

fn start_probe(
    config: kobo_sim::SimulationConfig,
) -> Result<(kobo_sim::AppSession, AppChild), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("locate matrix probe: {error}"))?;
    let mut command = Command::new(executable);
    command.arg("__matrix-probe").arg(config.scenario().name());
    start_process(command, config, false)
}

fn start_process(
    mut command: Command,
    config: kobo_sim::SimulationConfig,
    block_network: bool,
) -> Result<(kobo_sim::AppSession, AppChild), String> {
    reset_simulated_storage()?;
    let dev = DevSessionGuard::new()?;
    let server = kobo_sim::AppServer::bind_with_config("127.0.0.1:0", &dev.socket, config)
        .map_err(|error| format!("start simulator: {error}"))?;
    server
        .set_nonblocking(true)
        .map_err(|error| format!("configure simulator: {error}"))?;
    command.env("KOBO_SOCKET", &dev.socket);
    if block_network {
        command.env(kobo_sim::OFFLINE, "1");
    }
    let child = command
        .spawn()
        .map_err(|error| format!("launch simulator application: {error}"))?;
    let mut app = AppChild { child: Some(child) };
    let session = wait_for_app(&server, &mut app)?;
    wait_for_paint(&session, 0)?;
    wait_for_quiet(&session)?;
    // The socket is open and the reader thread owns it. Neither the HTTP
    // listener nor the private directory is needed by the headless path.
    drop(server);
    drop(dev);
    Ok((session, app))
}

fn reset_simulated_storage() -> Result<(), String> {
    for name in ["cobalt-sim-state", "cobalt-sim-data"] {
        let path = std::env::temp_dir().join(name);
        match fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("reset {}: {error}", path.display())),
        }
    }
    Ok(())
}

fn wait_for_quiet(session: &kobo_sim::AppSession) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut paints = session.paints();
    let mut unchanged_since = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        let current = session.paints();
        if current != paints {
            paints = current;
            unchanged_since = Instant::now();
        } else if unchanged_since.elapsed() >= Duration::from_millis(100) {
            return Ok(());
        }
    }
    Err("application did not settle within two seconds".to_owned())
}

fn wait_for_paint(session: &kobo_sim::AppSession, before: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while session.paints() <= before {
        if Instant::now() >= deadline {
            return Err("application did not paint within 10 seconds".to_owned());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn validate_session(session: &kobo_sim::AppSession) -> Result<(usize, Vec<u8>), String> {
    let diagnostics = session.diagnostics();
    let errors = diagnostics
        .issues
        .iter()
        .filter(|issue| format!("{:?}", issue.severity) == "Error")
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let config = session.config();
    let frame = session.ideal_frame();
    let expected = (config.pose().width() as usize)
        .checked_mul(config.pose().height() as usize)
        .ok_or("panel dimensions overflow")?;
    if frame.len() != expected {
        return Err(format!(
            "presentation frame has {} bytes, expected {expected}",
            frame.len()
        ));
    }
    match config.profile().framebuffer_packing() {
        Some(_) => {
            let packed = session.framebuffer_frame(true)?;
            let packed_expected = usize::try_from(config.profile().stride)
                .ok()
                .and_then(|stride| {
                    usize::try_from(config.pose().height())
                        .ok()
                        .and_then(|height| stride.checked_mul(height))
                })
                .ok_or("selected framebuffer is too large")?;
            if packed.len() != packed_expected {
                return Err(format!(
                    "framebuffer has {} bytes, expected {packed_expected}",
                    packed.len(),
                ));
            }
        }
        None => {
            if session.framebuffer_frame(true).is_ok() {
                return Err(
                    "unverified framebuffer packing was serialized instead of refused".to_owned(),
                );
            }
        }
    }
    let warnings = diagnostics
        .issues
        .iter()
        .filter(|issue| format!("{:?}", issue.severity) == "Warning")
        .count();
    Ok((warnings, frame))
}

fn scenario_case(config: kobo_sim::SimulationConfig) -> Result<(), String> {
    let (session, app) = start_probe(config)?;
    let result = (|| {
        let _ = validate_session(&session)?;
        let expected = format!("result:{}", scenario_expected(config.scenario()));
        if find_label(&session, &expected, false).is_none() {
            return Err(format!(
                "probe did not show {expected:?}; screen was {:?}",
                session
                    .layout()
                    .nodes
                    .iter()
                    .flat_map(|node| &node.text_lines)
                    .collect::<Vec<_>>()
            ));
        }
        let missing_first = session.diagnostics().issues.iter().any(|issue| {
            matches!(
                issue.kind,
                kobo_sdk::LayoutIssueKind::MissingPicture(kobo_sdk::PictureHandle(1))
            )
        });
        if missing_first != (config.scenario() == kobo_sim::Scenario::CachePressure) {
            return Err(
                "cache-pressure picture eviction did not match the selected scenario".into(),
            );
        }
        Ok(())
    })();
    let _ = session.close();
    drop(app);
    result
}

const fn scenario_expected(scenario: kobo_sim::Scenario) -> &'static str {
    match scenario {
        kobo_sim::Scenario::Normal => "battery-72",
        kobo_sim::Scenario::Offline => "offline",
        kobo_sim::Scenario::HostDown => "unreachable",
        kobo_sim::Scenario::LowBattery => "battery-5",
        kobo_sim::Scenario::PermissionDenied => "denied-not-declared",
        kobo_sim::Scenario::MissingSecret => "missing-secret",
        kobo_sim::Scenario::NetworkTimeout => "timed-out",
        kobo_sim::Scenario::StorageFull => "storage-too-full",
        kobo_sim::Scenario::CachePressure => "picture-evicted",
    }
}

const fn scenario_operation(scenario: kobo_sim::Scenario) -> &'static str {
    match scenario {
        kobo_sim::Scenario::Normal | kobo_sim::Scenario::LowBattery => "device.read-battery",
        kobo_sim::Scenario::Offline
        | kobo_sim::Scenario::HostDown
        | kobo_sim::Scenario::NetworkTimeout => "task.fetch",
        kobo_sim::Scenario::PermissionDenied => "applications.cached-catalog",
        kobo_sim::Scenario::MissingSecret => "task.post-with-credential",
        kobo_sim::Scenario::StorageFull => "store.save",
        kobo_sim::Scenario::CachePressure => "pictures.evict-and-render",
    }
}

fn drive_case(
    config: kobo_sim::SimulationConfig,
    screenshots: &Path,
) -> Result<Vec<Screenshot>, String> {
    let (session, app) = start_app("kobo-backgammon", config, true)?;
    let result = (|| {
        let mut shots = Vec::new();
        for line in DRIVE_ROUTE.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (verb, value) = line.split_once(' ').unwrap_or((line, ""));
            match verb {
                "expect" => {
                    if find_label(&session, value, false).is_none() {
                        return Err(format!("nothing on the screen says {value:?}"));
                    }
                }
                "tap" => {
                    let (x, y) = find_label(&session, value, true)
                        .ok_or_else(|| format!("no enabled action says {value:?}"))?;
                    let before = session.paints();
                    session
                        .touch(x, y)
                        .map_err(|error| format!("tap {value:?}: {error}"))?;
                    wait_for_paint(&session, before)?;
                    wait_for_quiet(&session)?;
                    let _ = validate_session(&session)?;
                }
                "shot" => {
                    let (_, frame) = validate_session(&session)?;
                    shots.push(write_shot(screenshots, config, value, &frame)?);
                }
                "clean" => {
                    let _ = validate_session(&session)?;
                }
                other => return Err(format!("unsupported drive step {other:?}")),
            }
        }
        let _ = validate_session(&session)?;
        Ok(shots)
    })();
    let _ = session.close();
    drop(app);
    result
}

fn find_label(session: &kobo_sim::AppSession, label: &str, actionable: bool) -> Option<(i32, i32)> {
    let needle = label.trim().to_lowercase();
    let layout = session.layout();
    for exact in [true, false] {
        for (index, labelled) in layout.nodes.iter().enumerate().filter(|(_, node)| {
            node.text_lines.iter().any(|line| {
                let line = line.trim().to_lowercase();
                if exact {
                    line == needle
                } else {
                    line.contains(&needle)
                }
            })
        }) {
            let target = if actionable {
                if labelled.kind.acts_on().is_some() {
                    labelled
                } else {
                    match layout
                        .nodes
                        .get(..index)
                        .into_iter()
                        .flatten()
                        .rev()
                        .find(|node| node.id == labelled.id && node.kind.acts_on().is_some())
                    {
                        Some(target) => target,
                        None => continue,
                    }
                }
            } else {
                labelled
            };
            return Some((
                target.rect.x + target.rect.width / 2,
                target.rect.y + target.rect.height / 2,
            ));
        }
    }
    None
}

fn write_shot(
    directory: &Path,
    config: kobo_sim::SimulationConfig,
    name: &str,
    frame: &[u8],
) -> Result<Screenshot, String> {
    let png = kobo_image::encode_png_grey(config.pose().width(), config.pose().height(), frame)
        .map_err(|error| format!("encode screenshot: {error}"))?;
    let path = directory.join(format!(
        "{}-r{}-{}.png",
        config.profile().id,
        config.pose().rotation(),
        name.replace(['/', ' '], "-")
    ));
    let sha256 = ring::digest::digest(&ring::digest::SHA256, &png);
    fs::write(&path, png).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(Screenshot {
        path: path.to_string_lossy().into_owned(),
        width: config.pose().width(),
        height: config.pose().height(),
        sha256: hex_digest(sha256.as_ref()),
    })
}

fn report_json(
    packages: &[&str],
    coverage: &BTreeMap<(&'static str, u32), Counts>,
    counts: &Counts,
    failures: &[Failure],
    screenshots: &[Screenshot],
) -> String {
    let mut json = format!(
        "{{\"schema\":1,\"protocols\":{{\"responsiveMatrix\":{},\
         \"legacyCompatibility\":{},\"legacyIncludedInResponsiveMatrix\":false}},\
         \"scenarioAssertions\":[",
        kobo_protocol::VERSION,
        kobo_protocol::LEGACY_VERSION,
    );
    for (index, scenario) in kobo_sim::Scenario::ALL.into_iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"name\":{},\"operation\":{},\"expected\":{}}}",
            quote(scenario.name()),
            quote(scenario_operation(scenario)),
            quote(scenario_expected(scenario))
        );
    }
    json.push_str("],\"profiles\":[");
    for (index, ((profile, rotation), count)) in coverage.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"id\":{},\"rotation\":{},\"initial\":{},\"scenarios\":{},\"drives\":{},\"warnings\":{}}}",
            quote(profile),
            rotation,
            count.initial,
            count.scenarios,
            count.drives,
            count.warnings
        );
    }
    json.push_str("],\"apps\":[");
    for (index, package) in packages.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&quote(package));
    }
    let total = counts.initial + counts.scenarios + counts.drives;
    let _ = write!(
        json,
        "],\"counts\":{{\"initial\":{},\"scenarios\":{},\"drives\":{},\"total\":{},\"passed\":{},\"failed\":{},\"warnings\":{},\"screenshots\":{}}},\"failures\":[",
        counts.initial,
        counts.scenarios,
        counts.drives,
        total,
        total.saturating_sub(failures.len()),
        failures.len(),
        counts.warnings,
        screenshots.len()
    );
    for (index, failure) in failures.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"kind\":{},\"profile\":{},\"rotation\":{},\"subject\":{},\"error\":{}}}",
            quote(&failure.kind),
            quote(&failure.profile),
            failure.rotation,
            quote(&failure.subject),
            quote(&failure.error)
        );
    }
    json.push_str("],\"screenshots\":[");
    for (index, screenshot) in screenshots.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"path\":{},\"width\":{},\"height\":{},\"sha256\":{}}}",
            quote(&screenshot.path),
            screenshot.width,
            screenshot.height,
            quote(&screenshot.sha256)
        );
    }
    json.push_str("]}\n");
    json
}

fn quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                let _ = write!(quoted, "\\u{:04x}", character as u32);
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{
        packages, prepare_screenshot_directory, report_json, run, scenario_expected,
        scenario_operation, validate_output_paths, Counts, Screenshot,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn matrix_covers_the_union_of_platform_and_store_apps() {
        let packages = packages();
        assert_eq!(packages.len(), 20);
        assert!(packages.contains(&"kobo-launcher"));
        assert!(packages.contains(&"kobo-store"));
        assert!(packages.contains(&"kobo-backgammon"));
        assert!(!packages.contains(&"kobod"));
    }

    #[test]
    fn responsive_and_legacy_protocols_are_reported_separately() {
        let json = report_json(&[], &BTreeMap::new(), &Counts::default(), &[], &[]);
        assert!(json.contains("\"responsiveMatrix\":12"));
        assert!(json.contains("\"legacyCompatibility\":11"));
        assert!(json.contains("\"legacyIncludedInResponsiveMatrix\":false"));
    }

    #[test]
    fn every_scenario_has_one_asserted_service_result() {
        let results = kobo_sim::Scenario::ALL
            .map(scenario_expected)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(results.len(), kobo_sim::Scenario::ALL.len());
        assert!(!results.contains("pending"));
        assert_eq!(
            scenario_operation(kobo_sim::Scenario::CachePressure),
            "pictures.evict-and-render"
        );
        let json = report_json(&[], &BTreeMap::new(), &Counts::default(), &[], &[]);
        assert!(json.contains(
            "\"name\":\"storage-full\",\"operation\":\"store.save\",\
             \"expected\":\"storage-too-full\""
        ));
    }

    #[test]
    fn screenshot_report_is_self_inventorying() {
        let screenshot = Screenshot {
            path: "/artifacts/libra.png".to_owned(),
            width: 1264,
            height: 1680,
            sha256: "a".repeat(64),
        };
        let json = report_json(
            &[],
            &BTreeMap::new(),
            &Counts::default(),
            &[],
            &[screenshot],
        );
        assert!(json.contains("\"screenshots\":1"));
        assert!(json.contains("\"path\":\"/artifacts/libra.png\""));
        assert!(json.contains("\"width\":1264,\"height\":1680"));
        assert!(json.contains(&format!("\"sha256\":\"{}\"", "a".repeat(64))));
    }

    #[test]
    fn screenshot_directory_must_be_absent_or_empty() {
        let directory = std::env::temp_dir().join(format!(
            "km-empty-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        prepare_screenshot_directory(&directory).expect("absent directory is created");
        prepare_screenshot_directory(&directory).expect("empty directory is accepted");
        fs::write(directory.join("stale.png"), b"stale").expect("write stale artifact");
        assert!(prepare_screenshot_directory(&directory).is_err());
        fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[test]
    fn nested_report_path_is_rejected_before_generating_anything() {
        let screenshots = std::env::temp_dir().join(format!(
            "km-nested-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let report = screenshots.join("nested").join("..").join("matrix.json");
        let _ = fs::remove_dir_all(&screenshots);
        let arguments = vec![
            "--report".to_owned(),
            report.to_string_lossy().into_owned(),
            "--screenshots".to_owned(),
            screenshots.to_string_lossy().into_owned(),
            "--skip-build".to_owned(),
        ];
        assert!(run(&arguments)
            .expect_err("nested report must be refused")
            .contains("must be outside screenshot directory"));
        assert!(!screenshots.exists());
    }

    #[cfg(unix)]
    #[test]
    fn canonical_equivalent_report_path_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "km-canonical-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let actual = root.join("actual");
        let screenshots = actual.join("screenshots");
        fs::create_dir_all(&screenshots).expect("create screenshot directory");
        let alias = root.join("alias");
        symlink(&actual, &alias).expect("create path alias");
        assert!(validate_output_paths(
            &screenshots.join("matrix.json"),
            &alias.join("screenshots")
        )
        .expect_err("canonical equivalent report must be refused")
        .contains("must be outside screenshot directory"));
        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_report_symlink_into_screenshots_is_rejected_without_artifacts() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "km-dangling-report-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let outside = root.join("outside");
        let screenshots = root.join("screenshots");
        let report = outside.join("report.json");
        fs::create_dir_all(&outside).expect("create outside directory");
        symlink("../screenshots/report.json", &report).expect("create dangling report symlink");
        let arguments = vec![
            "--report".to_owned(),
            report.to_string_lossy().into_owned(),
            "--screenshots".to_owned(),
            screenshots.to_string_lossy().into_owned(),
            "--skip-build".to_owned(),
        ];
        assert!(run(&arguments)
            .expect_err("dangling report symlink must be refused")
            .contains("contains dangling symlink"));
        assert!(!screenshots.exists());
        assert!(fs::symlink_metadata(&report)
            .expect("report fixture remains")
            .file_type()
            .is_symlink());
        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_screenshot_symlink_is_rejected_without_artifacts() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "km-dangling-screenshots-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let report = root.join("report.json");
        let screenshots = root.join("screenshots");
        let target = root.join("missing-screenshots");
        fs::create_dir_all(&root).expect("create fixture directory");
        symlink(&target, &screenshots).expect("create dangling screenshot symlink");
        let arguments = vec![
            "--report".to_owned(),
            report.to_string_lossy().into_owned(),
            "--screenshots".to_owned(),
            screenshots.to_string_lossy().into_owned(),
            "--skip-build".to_owned(),
        ];
        assert!(run(&arguments)
            .expect_err("dangling screenshot symlink must be refused")
            .contains("contains dangling symlink"));
        assert!(!report.exists());
        assert!(!target.exists());
        fs::remove_dir_all(root).expect("remove test directory");
    }
}
