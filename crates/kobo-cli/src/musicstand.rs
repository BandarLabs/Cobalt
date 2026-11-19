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
const USAGE: &str = concat!(
    "usage: kobo musicstand push SCORE... ",
    crate::targets::target_choice!(),
    " [--replace]\n\
                     \x20      kobo musicstand plan SCORE... ",
    crate::targets::target_choice!(),
    " [--replace]\n\
                     \x20      kobo musicstand ls ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo musicstand rm ID ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo musicstand init ",
    crate::targets::target_choice!(),
    "\n\
                     \n\
                     A SCORE is a PDF, a PNG or JPEG page, or a folder. Each PDF in a\n\
                     folder becomes its own score; the page images directly in a\n\
                     folder become one score named after it. A push adds to the\n\
                     library; --replace makes the library exactly this push.\n\
                     PDFs need Poppler's pdftoppm: brew install poppler on a Mac,\n\
                     sudo apt install poppler-utils on Debian or Ubuntu.\n",
    crate::targets::saved_reader_note!(),
);
/// What an owner without Poppler is told, before anything is read or sent.
const NO_POPPLER: &str = "Music Stand needs Poppler to prepare PDF scores, and pdftoppm \
                          is not installed on this computer. Install it with brew install \
                          poppler on a Mac, or sudo apt install poppler-utils on Debian or \
                          Ubuntu, then push again.";

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

#[derive(Debug, Eq, PartialEq)]
struct PushOptions {
    inputs: Vec<String>,
    target: Target,
    replace: bool,
}

fn parse_push(arguments: &[String]) -> Result<PushOptions, String> {
    let mut inputs = Vec::new();
    let mut target = None;
    let mut replace = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--sim" => {
                if target.is_some() {
                    return Err(USAGE.to_owned());
                }
                target = Some(Target::Sim);
                index += 1;
            }
            flag if super::is_device_flag(flag) => {
                if target.is_some() {
                    return Err(USAGE.to_owned());
                }
                let host = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                if !super::valid_device_host(host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                target = Some(Target::Device(host.clone()));
                index += 2;
            }
            "--replace" => {
                replace = true;
                index += 1;
            }
            flag if flag.starts_with('-') => return Err(USAGE.to_owned()),
            input => {
                inputs.push(input.to_owned());
                index += 1;
            }
        }
    }
    let target = target.ok_or_else(|| USAGE.to_owned())?;
    if inputs.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(PushOptions {
        inputs,
        target,
        replace,
    })
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

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// The files directly in a folder, in name order.
fn folder_entries(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(folder)
        .map_err(|error| format!("read {}: {error}", folder.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    Ok(paths)
}

/// One score's worth of input: a PDF, a single page image, or a folder whose
/// page images make one score together.
#[derive(Debug, Eq, PartialEq)]
enum Source {
    File(PathBuf),
    Pages(PathBuf),
}

/// Turns what was named on the command line into scores.
///
/// A folder of page images has always been one score, and stays one. A
/// folder of PDFs is a folder of scores, which is how most people keep a
/// repertoire, so each PDF in it becomes its own; asking for them one push at
/// a time was the reason a library took an evening to fill.
fn sources(inputs: &[String]) -> Result<Vec<Source>, String> {
    let mut found = Vec::new();
    for input in inputs {
        let path = PathBuf::from(input);
        if path.is_dir() {
            let entries = folder_entries(&path)?;
            found.extend(
                entries
                    .iter()
                    .filter(|entry| is_pdf(entry))
                    .cloned()
                    .map(Source::File),
            );
            if entries.iter().any(|entry| is_image(entry)) {
                found.push(Source::Pages(path));
            }
        } else if path.exists() {
            found.push(Source::File(path));
        } else {
            return Err(format!("{input} does not exist"));
        }
    }
    if found.is_empty() {
        return Err(
            "nothing to send: name a PDF, a page image, or a folder holding them".to_owned(),
        );
    }
    Ok(found)
}

/// Checks for Poppler before a PDF is read or the reader is asked for
/// anything, so an owner without it hears the one thing to install rather
/// than a failure per score after a wait.
fn require_poppler(sources: &[Source]) -> Result<(), String> {
    let needs = sources
        .iter()
        .any(|source| matches!(source, Source::File(path) if is_pdf(path)));
    if !needs {
        return Ok(());
    }
    match Command::new("pdftoppm").arg("-v").output() {
        Ok(_) => Ok(()),
        Err(_) => Err(NO_POPPLER.to_owned()),
    }
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
        .map_err(|_| NO_POPPLER.to_owned())?;
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

fn offer(
    input: &Path,
    panel: Panel,
    folder_of_pages: bool,
) -> Result<IncomingScore, Vec<ImportFailure>> {
    let name = if folder_of_pages {
        input.file_name()
    } else {
        input.file_stem()
    };
    let title = name
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

fn print_plan(push: &Push, existing: &Manifest) {
    for score in &push.manifest.scores {
        let state = if existing.scores.iter().any(|old| old.id == score.id) {
            "keep"
        } else {
            "add"
        };
        println!(
            "{state} {} · {} · {} page(s)",
            score.id, score.title, score.pages
        );
    }
    for removed in &push.removed {
        println!("remove {} · {}", removed.id, removed.title);
    }
    for failure in &push.manifest.failures {
        println!("could not import {}: {}", failure.input, failure.reason);
    }
}

/// What the library holds after a push, so the command ends by naming what
/// the reader will show.
fn on_the_reader(manifest: &Manifest) -> String {
    let count = manifest.scores.len();
    let mut text = format!(
        "Now on the reader: {count} score{}.\n",
        if count == 1 { "" } else { "s" }
    );
    for score in &manifest.scores {
        let plural = if score.pages == 1 { "" } else { "s" };
        let _ = writeln!(text, "  {} ({} page{plural})", score.title, score.pages);
    }
    text
}

fn push(arguments: &[String], plan_only: bool) -> Result<(), String> {
    let options = parse_push(arguments)?;
    let sources = sources(&options.inputs)?;
    require_poppler(&sources)?;
    let target = options.target;
    let (existing, panel) = match &target {
        Target::Device(host) => (read_manifest(host)?, reader_panel(host)?),
        Target::Sim => (read_local_manifest()?, sim_panel()),
    };
    let mut incoming = Vec::new();
    let mut failures = Vec::new();
    for source in &sources {
        let (path, title) = match source {
            Source::File(path) => (path.as_path(), None),
            Source::Pages(folder) => (folder.as_path(), Some(folder)),
        };
        match offer(path, panel, title.is_some()) {
            Ok(offer) => incoming.push(offer),
            Err(mut failed) => failures.append(&mut failed),
        }
    }
    if incoming.is_empty() {
        for failure in &failures {
            eprintln!("could not import {}: {}", failure.input, failure.reason);
        }
        return Err("Nothing could be prepared, so nothing was sent.".to_owned());
    }
    let push = plan(&existing, incoming, failures, options.replace)?;
    print_plan(&push, &existing);
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
        "Sent {} new page file(s) and removed {} score(s).",
        push.pages.len(),
        push.removed.len()
    );
    print!("{}", on_the_reader(&push.manifest));
    println!("Music Stand shows new scores the next time you come back to it.");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn a_push_takes_several_scores_and_adds_unless_asked_to_replace() {
        let options = parse_push(&arguments(&["one.pdf", "two.pdf", "--sim"])).expect("parse");
        assert_eq!(options.inputs, ["one.pdf", "two.pdf"]);
        assert!(!options.replace);
        let options = parse_push(&arguments(&[
            "one.pdf",
            "--replace",
            "--device",
            "kobo.local",
        ]))
        .expect("parse");
        assert!(options.replace);
        assert_eq!(options.target, Target::Device("kobo.local".to_owned()));
        assert!(parse_push(&arguments(&["--sim"])).is_err());
        assert!(parse_push(&arguments(&["one.pdf", "--sim", "--sim"])).is_err());
    }

    #[test]
    fn a_folder_of_pdfs_is_a_folder_of_scores_and_its_page_images_are_one_more() {
        let root = std::env::temp_dir().join(format!("musicstand-sources-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("folder");
        for name in [
            "Prelude.pdf",
            "Gigue.PDF",
            "page-1.png",
            "page-2.jpg",
            "notes.txt",
        ] {
            fs::write(root.join(name), b"x").expect("file");
        }
        let found = sources(&[root.display().to_string()]).expect("sources");
        assert_eq!(
            found,
            [
                Source::File(root.join("Gigue.PDF")),
                Source::File(root.join("Prelude.pdf")),
                Source::Pages(root.clone()),
            ]
        );
        assert!(sources(&[root.join("missing.pdf").display().to_string()]).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_push_without_pdfs_never_asks_for_poppler() {
        assert!(require_poppler(&[Source::File(PathBuf::from("page.png"))]).is_ok());
        assert!(require_poppler(&[Source::Pages(PathBuf::from("pages"))]).is_ok());
    }

    #[test]
    fn missing_poppler_names_the_package_to_install_on_each_system() {
        assert!(NO_POPPLER.contains("brew install poppler"));
        assert!(NO_POPPLER.contains("poppler-utils"));
    }

    #[test]
    fn a_push_ends_by_naming_every_score_on_the_reader() {
        let manifest = Manifest {
            scores: vec![kobo_music_host::Score {
                id: "score-a".to_owned(),
                title: "Prelude".to_owned(),
                digest: "d".to_owned(),
                pages: 3,
                width: 2144,
                height: 3000,
                added: 1,
            }],
            failures: Vec::new(),
        };
        assert_eq!(
            on_the_reader(&manifest),
            "Now on the reader: 1 score.\n  Prelude (3 pages)\n"
        );
    }
}
