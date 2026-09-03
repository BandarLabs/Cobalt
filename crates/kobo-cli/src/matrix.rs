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

#[derive(Debug)]
struct Failure {
    kind: String,
    profile: String,
    rotation: u32,
    subject: String,
    error: String,
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

pub fn run(arguments: &[String]) -> Result<(), String> {
    if matches!(arguments, [help] if matches!(help.as_str(), "--help" | "-h")) {
        println!("{USAGE}");
        return Ok(());
    }
    let options = parse(arguments)?;
    let packages = packages();
    if !options.skip_build {
        build(&packages)?;
    }
    fs::create_dir_all(&options.screenshots)
        .map_err(|error| format!("create {}: {error}", options.screenshots.display()))?;
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
    }

    finish_report(packages, &configs, report, &counts, &coverage, &failures)
}

fn execute(
    packages: &[&str],
    configs: &[kobo_sim::SimulationConfig],
    report: &Path,
    screenshots: &Path,
) -> Result<(), String> {
    let mut counts = Counts::default();
    let mut failures = Vec::new();
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
                        write_shot(screenshots, config, "store-initial", &frame)?;
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
        if let Err(error) = drive_case(config, screenshots) {
            failures.push(Failure {
                kind: "drive".to_owned(),
                profile: config.profile().id.to_owned(),
                rotation: config.pose().rotation(),
                subject: "apps/backgammon/drive.txt".to_owned(),
                error,
            });
        }
    }

    finish_report(packages, configs, report, &counts, &coverage, &failures)
}

fn finish_report(
    packages: &[&str],
    configs: &[kobo_sim::SimulationConfig],
    report: &Path,
    counts: &Counts,
    coverage: &BTreeMap<(&'static str, u32), Counts>,
    failures: &[Failure],
) -> Result<(), String> {
    let json = report_json(packages, coverage, counts, failures);
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

fn read_worker_report(path: &Path) -> Result<(Counts, Vec<Failure>), String> {
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
    Ok((counts, failures))
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
    reset_simulated_storage()?;
    let dev = DevSessionGuard::new()?;
    let server = kobo_sim::AppServer::bind_with_config("127.0.0.1:0", &dev.socket, config)
        .map_err(|error| format!("start simulator: {error}"))?;
    server
        .set_nonblocking(true)
        .map_err(|error| format!("configure simulator: {error}"))?;
    let mut command = Command::new(workspace_host_binary(package));
    command.env("KOBO_SOCKET", &dev.socket);
    if block_network {
        command.env(kobo_sim::OFFLINE, "1");
    }
    let child = command
        .spawn()
        .map_err(|error| format!("launch {package}: {error}"))?;
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
    let mut simulator = kobo_sim::Simulator::with_config(config);
    let frame = simulator.ideal_frame();
    let expected = (config.pose().width() as usize)
        .checked_mul(config.pose().height() as usize)
        .ok_or("panel dimensions overflow")?;
    if frame.len() != expected {
        return Err("scenario frame size does not match the selected profile".to_owned());
    }
    let errors = simulator
        .diagnostics()
        .issues
        .into_iter()
        .filter(|issue| format!("{:?}", issue.severity) == "Error")
        .map(|issue| issue.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn drive_case(config: kobo_sim::SimulationConfig, screenshots: &Path) -> Result<(), String> {
    let (session, app) = start_app("kobo-backgammon", config, true)?;
    let result = (|| {
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
                    write_shot(screenshots, config, value, &frame)?;
                }
                "clean" => {
                    let _ = validate_session(&session)?;
                }
                other => return Err(format!("unsupported drive step {other:?}")),
            }
        }
        let _ = validate_session(&session)?;
        Ok(())
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
) -> Result<(), String> {
    let png = kobo_image::encode_png_grey(config.pose().width(), config.pose().height(), frame)
        .map_err(|error| format!("encode screenshot: {error}"))?;
    let path = directory.join(format!(
        "{}-r{}-{}.png",
        config.profile().id,
        config.pose().rotation(),
        name.replace(['/', ' '], "-")
    ));
    fs::write(&path, png).map_err(|error| format!("write {}: {error}", path.display()))
}

fn report_json(
    packages: &[&str],
    coverage: &BTreeMap<(&'static str, u32), Counts>,
    counts: &Counts,
    failures: &[Failure],
) -> String {
    let mut json = format!(
        "{{\"schema\":1,\"protocols\":{{\"responsiveMatrix\":{},\
         \"legacyCompatibility\":{},\"legacyIncludedInResponsiveMatrix\":false}},\
         \"profiles\":[",
        kobo_protocol::VERSION,
        kobo_protocol::LEGACY_VERSION,
    );
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
        "],\"counts\":{{\"initial\":{},\"scenarios\":{},\"drives\":{},\"total\":{},\"passed\":{},\"failed\":{},\"warnings\":{}}},\"failures\":[",
        counts.initial,
        counts.scenarios,
        counts.drives,
        total,
        total.saturating_sub(failures.len()),
        failures.len(),
        counts.warnings
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
    use super::{packages, report_json, Counts};
    use std::collections::BTreeMap;

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
        let json = report_json(&[], &BTreeMap::new(), &Counts::default(), &[]);
        assert!(json.contains("\"responsiveMatrix\":12"));
        assert!(json.contains("\"legacyCompatibility\":11"));
        assert!(json.contains("\"legacyIncludedInResponsiveMatrix\":false"));
    }
}
