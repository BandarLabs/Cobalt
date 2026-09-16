//! Owner-attended Music Stand shelf management: prepare scores on the host
//! and publish them over the already-paired SSH channel or into the simulator.

use kobo_music_host::{
    digest, fit_page, plan, ImportFailure, IncomingScore, Manifest, Panel, Push, MANIFEST,
    MAX_SCORES,
};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/musicstand";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;
const USAGE: &str = "usage: kobo musicstand init (--sim | --device IP)\n\
                     \x20      kobo musicstand push SCORE.pdf|IMAGES_DIR (--sim | --device IP)\n\
                     \x20      kobo musicstand plan SCORE.pdf|IMAGES_DIR (--sim | --device IP)\n\
                     \x20      kobo musicstand ls (--sim | --device IP)\n\
                     \x20      kobo musicstand rm ID (--sim | --device IP)";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Device(String),
    Sim,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("init") => init(&arguments[1..]),
        Some("push") => push(&arguments[1..], false),
        Some("plan") => push(&arguments[1..], true),
        Some("ls") => list(&arguments[1..]),
        Some("rm") => remove(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_target(arguments: &[String]) -> Result<Target, String> {
    match arguments {
        [flag] if flag == "--sim" => Ok(Target::Sim),
        [flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok(Target::Device(host.clone()))
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_push(arguments: &[String]) -> Result<(String, Target), String> {
    match arguments {
        [input, flag] if flag == "--sim" => Ok((input.clone(), Target::Sim)),
        [input, flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok((input.clone(), Target::Device(host.clone())))
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    match parse_target(arguments)? {
        Target::Device(host) => {
            let output = remote(
                &host,
                &format!(
                    "set -eu\nmkdir -p '{ROOT}'\nchmod 700 '{ROOT}'\nsync\nprintf 'Music Stand shelf ready; transfers use this owner-attended SSH connection.\\n'\n"
                ),
            )?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        Target::Sim => {
            let root = sim_root();
            fs::create_dir_all(&root)
                .map_err(|error| format!("create Music Stand simulator shelf: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                    .map_err(|error| format!("protect Music Stand simulator shelf: {error}"))?;
            }
            println!(
                "Music Stand shelf ready at {}; transfers stay on this computer.",
                root.display()
            );
        }
    }
    Ok(())
}

fn bounded_file(path: &Path) -> Result<Vec<u8>, String> {
    let size = fs::metadata(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .len();
    if size > MAX_INPUT_BYTES {
        return Err(format!(
            "{} is too large for Music Stand ({} byte limit)",
            path.display(),
            MAX_INPUT_BYTES
        ));
    }
    fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg"
            )
        })
}

/// Rasterize a PDF with Poppler into a temporary directory of PNG pages.
fn pdf_pages(input: &Path, temporary: &Path) -> Result<Vec<PathBuf>, String> {
    let prefix = temporary.join("page");
    let output = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("200")
        .arg(input)
        .arg(&prefix)
        .output()
        .map_err(|error| {
            format!("could not start pdftoppm ({error}); install Poppler to prepare this PDF")
        })?;
    if !output.status.success() {
        return Err(format!(
            "pdftoppm could not rasterize {}; it may be encrypted or malformed",
            input.display()
        ));
    }
    let mut pages: Vec<PathBuf> = fs::read_dir(temporary)
        .map_err(|error| format!("read rasterized pages: {error}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_image(path))
        .collect();
    pages.sort();
    Ok(pages)
}

/// Walk the input into one offer: a PDF becomes a score of rasterized pages,
/// an image directory becomes a score of its images in name order, and one
/// image becomes a one-page score.
/// Walk the input into page files: a PDF is rasterized into a staging
/// directory, an image directory becomes its images in name order, and one
/// image becomes a one-page score.
fn collect_page_paths(
    input: &Path,
    pages_dir: &mut Option<PathBuf>,
) -> Result<Vec<PathBuf>, Vec<ImportFailure>> {
    if input.is_dir() {
        let mut paths: Vec<PathBuf> = match fs::read_dir(input) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| is_image(path))
                .collect(),
            Err(error) => {
                return Err(vec![ImportFailure {
                    input: input.display().to_string(),
                    reason: format!("could not be read: {error}"),
                }]);
            }
        };
        paths.sort();
        return Ok(paths);
    }
    if input
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        let staged = std::env::temp_dir().join(format!("cobalt-musicstand-{}", std::process::id()));
        let _ = fs::remove_dir_all(&staged);
        if let Err(error) = fs::create_dir_all(&staged) {
            return Err(vec![ImportFailure {
                input: input.display().to_string(),
                reason: format!("could not stage rasterized pages: {error}"),
            }]);
        }
        *pages_dir = Some(staged.clone());
        return match pdf_pages(input, &staged) {
            Ok(paths) => Ok(paths),
            Err(reason) => Err(vec![ImportFailure {
                input: input.display().to_string(),
                reason,
            }]),
        };
    }
    if is_image(input) {
        return Ok(vec![input.to_path_buf()]);
    }
    Err(vec![ImportFailure {
        input: input.display().to_string(),
        reason: "not a PDF or a PNG/JPEG image".to_owned(),
    }])
}

fn offer(input: &Path, panel: Panel) -> Result<IncomingScore, Vec<ImportFailure>> {
    let title = input
        .file_stem()
        .and_then(|name| name.to_str())
        .map_or_else(|| "Untitled score".to_owned(), str::to_owned);
    let mut failures = Vec::new();
    let mut pages_dir: Option<PathBuf> = None;
    let page_paths: Vec<PathBuf> = collect_page_paths(input, &mut pages_dir)?;
    let mut pages = Vec::new();
    let mut source_bytes = Vec::new();
    for path in &page_paths {
        match bounded_file(path) {
            Ok(bytes) => {
                source_bytes.extend_from_slice(&bytes);
                match kobo_image::decode(&bytes) {
                    Ok(picture) => match fit_page(&picture, panel) {
                        Ok(page) => pages.push(page),
                        Err(reason) => failures.push(ImportFailure {
                            input: path.display().to_string(),
                            reason,
                        }),
                    },
                    Err(error) => failures.push(ImportFailure {
                        input: path.display().to_string(),
                        reason: format!("could not be decoded: {error}"),
                    }),
                }
            }
            Err(reason) => failures.push(ImportFailure {
                input: path.display().to_string(),
                reason,
            }),
        }
    }
    if let Some(staged) = pages_dir {
        let _ = fs::remove_dir_all(staged);
    }
    if pages.is_empty() {
        if failures.is_empty() {
            failures.push(ImportFailure {
                input: input.display().to_string(),
                reason: "no readable pages".to_owned(),
            });
        }
        return Err(failures);
    }
    let added = fs::metadata(input)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs());
    Ok(IncomingScore {
        title,
        digest: digest(&source_bytes),
        added,
        pages,
    })
}

fn sim_root() -> PathBuf {
    kobo_sim::simulated_data_root("musicstand")
}

fn sim_panel() -> Panel {
    let profile = kobo_sim::selected_profile();
    Panel {
        width: profile.width,
        height: profile.height,
    }
}

fn read_local_manifest() -> Result<Manifest, String> {
    let path = sim_root().join(MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("the simulator Music Stand manifest is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(error) => Err(format!("read Music Stand simulator shelf: {error}")),
    }
}

fn publish_local(push: &Push) -> Result<(), String> {
    let root = sim_root();
    fs::create_dir_all(&root)
        .map_err(|error| format!("create Music Stand simulator shelf: {error}"))?;
    for prepared in &push.pages {
        let name = Push::page_name(&prepared.score_id, prepared.index);
        let dest = root.join(&name);
        let partial = root.join(format!(".{name}.writing"));
        fs::write(&partial, &prepared.png)
            .map_err(|error| format!("write Music Stand page {name}: {error}"))?;
        fs::rename(&partial, &dest)
            .map_err(|error| format!("publish Music Stand page {name}: {error}"))?;
    }
    let dest = root.join(MANIFEST);
    let partial = root.join(format!(".{MANIFEST}.writing"));
    fs::write(&partial, push.manifest.encode())
        .map_err(|error| format!("write Music Stand manifest: {error}"))?;
    fs::rename(&partial, &dest)
        .map_err(|error| format!("publish Music Stand manifest: {error}"))?;
    for removed in &push.removed {
        for index in 0..removed.pages {
            let _ = fs::remove_file(root.join(Push::page_name(&removed.id, index)));
        }
    }
    Ok(())
}

fn print_plan(push: &Push) {
    for score in &push.manifest.scores {
        println!(
            "score {} · {} · {} page(s) · {}x{}",
            score.id, score.title, score.pages, score.width, score.height
        );
    }
    for removed in &push.removed {
        println!("remove {} · {}", removed.id, removed.title);
    }
    for failure in &push.manifest.failures {
        println!("could not import {}: {}", failure.input, failure.reason);
    }
}

fn push(arguments: &[String], plan_only: bool) -> Result<(), String> {
    let (input, target) = parse_push(arguments)?;
    let (existing, panel) = match &target {
        Target::Device(host) => (read_manifest(host)?, reader_panel(host)?),
        Target::Sim => (read_local_manifest()?, sim_panel()),
    };
    let incoming = match offer(Path::new(&input), panel) {
        Ok(offer) => vec![offer],
        Err(failures) => {
            for failure in &failures {
                eprintln!("could not import {}: {}", failure.input, failure.reason);
            }
            return Err("nothing readable to publish".to_owned());
        }
    };
    let push = plan(&existing, incoming, Vec::new())?;
    print_plan(&push);
    if push.manifest.scores.len() > MAX_SCORES {
        return Err(format!("Music Stand holds at most {MAX_SCORES} scores"));
    }
    if plan_only {
        println!("Plan only. No pages were transferred or removed.");
        return Ok(());
    }
    match &target {
        Target::Device(host) => {
            transfer(host, &push)?;
            let actual = read_manifest(host)?;
            if actual.scores != push.manifest.scores {
                return Err(
                    "the reader's Music Stand shelf does not match the published plan".to_owned(),
                );
            }
        }
        Target::Sim => {
            publish_local(&push)?;
            let actual = read_local_manifest()?;
            if actual.scores != push.manifest.scores {
                return Err(
                    "the simulator Music Stand shelf does not match the published plan".to_owned(),
                );
            }
        }
    }
    println!(
        "Published {} score(s) to the Music Stand shelf.",
        push.manifest.scores.len()
    );
    Ok(())
}

fn list(arguments: &[String]) -> Result<(), String> {
    let manifest = match parse_target(arguments)? {
        Target::Device(host) => read_manifest(&host)?,
        Target::Sim => read_local_manifest()?,
    };
    if manifest.scores.is_empty() {
        println!("The Music Stand shelf is empty.");
    }
    for score in &manifest.scores {
        println!("{} · {} · {} page(s)", score.id, score.title, score.pages);
    }
    for failure in &manifest.failures {
        println!("could not import {}: {}", failure.input, failure.reason);
    }
    Ok(())
}

fn remove(arguments: &[String]) -> Result<(), String> {
    let (id, target) = match arguments {
        [id, flag] if flag == "--sim" => (id.clone(), Target::Sim),
        [id, flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            (id.clone(), Target::Device(host.clone()))
        }
        _ => return Err(USAGE.to_owned()),
    };
    let mut manifest = match &target {
        Target::Device(host) => read_manifest(host)?,
        Target::Sim => read_local_manifest()?,
    };
    let Some(position) = manifest.scores.iter().position(|score| score.id == id) else {
        return Err(format!("no Music Stand score named {id}"));
    };
    let removed = manifest.scores.remove(position);
    let push = Push {
        manifest,
        pages: Vec::new(),
        removed: vec![removed.clone()],
    };
    match &target {
        Target::Device(host) => transfer(host, &push)?,
        Target::Sim => publish_local(&push)?,
    }
    println!("Removed {} from the Music Stand shelf.", removed.title);
    Ok(())
}

fn read_manifest(host: &str) -> Result<Manifest, String> {
    let output = remote(
        host,
        &format!("cat '{ROOT}/{MANIFEST}' 2>/dev/null || true\n"),
    )?;
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(Manifest::default());
    }
    Manifest::decode(text.trim().as_bytes())
        .map_err(|error| format!("the reader's Music Stand manifest is invalid: {error}"))
}

fn reader_panel(host: &str) -> Result<Panel, String> {
    let profile = kobo_sim::selected_profile();
    let output = remote(host, "cat /mnt/onboard/.kobo/version 2>/dev/null || true\n")?;
    let _ = output;
    Ok(Panel {
        width: profile.width,
        height: profile.height,
    })
}

fn transfer(host: &str, push: &Push) -> Result<(), String> {
    let mut script = format!("set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\n");
    for prepared in &push.pages {
        let name = Push::page_name(&prepared.score_id, prepared.index);
        let encoded = super::base64_encode(&prepared.png);
        let _ = write!(
            script,
            "partial=\"$root/.{name}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_MUSIC_PAGE'\n{encoded}\nCOBALT_MUSIC_PAGE\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{name}\"\n"
        );
    }
    let encoded = super::base64_encode(&push.manifest.encode());
    let _ = write!(
        script,
        "partial=\"$root/.{MANIFEST}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_MUSIC_MANIFEST'\n{encoded}\nCOBALT_MUSIC_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{MANIFEST}\"\nsync\n"
    );
    for removed in &push.removed {
        for index in 0..removed.pages {
            let _ = writeln!(
                script,
                "rm -f \"$root/{}\"",
                Push::page_name(&removed.id, index)
            );
        }
    }
    script.push_str("sync\n");
    let _ = remote(host, &script)?;
    Ok(())
}

fn remote(host: &str, script: &str) -> Result<super::RemoteShellOutput, String> {
    let output = super::run_remote_shell(&format!("root@{host}"), script, TRANSFER_TIMEOUT)
        .map_err(super::unreachable_device)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(super::unreachable_if_ssh_gave_up(
            super::remote_shell_error(
                format!(
                    "Music Stand transfer on {host} exited with {}",
                    output.status
                ),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ))
    }
}
