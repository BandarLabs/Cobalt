//! Owner-attended preparation and transfer for Needles pattern documents.
//!
//! PDF parsing belongs on the host: this keeps the reader application small,
//! lets the shared book reader handle reflow, and never sends credentials here.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::process::{Command, Stdio};

const MAX_PDF: usize = 32 * 1024 * 1024;
const MAX_PATTERN: usize = 4 * 1024 * 1024;
const BLOB: &str = "pattern.md";
/// How much of the extracted pattern a printed preview shows against its source.
const PREVIEW_LINES: usize = 12;
const USAGE: &str = "usage: kobo needles converter status|install\n\
                     \x20      kobo needles setup\n\
                     \x20      kobo needles prepare PATTERN.(pdf|md|txt) --out PATTERN.md [--title TITLE] [--section HEADING]\n\
                     \x20      kobo needles preview PATTERN.(pdf|md|txt) [--out DIRECTORY] [--title TITLE] [--section HEADING]\n\
                     \x20      kobo needles push PATTERN.(pdf|md|txt) (--sim | --device IP | --out FILE) [--title TITLE] [--section HEADING]";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    if arguments.first().map(String::as_str) == Some("converter") {
        return converter_command(&arguments[1..]);
    }
    if arguments.first().map(String::as_str) == Some("setup") {
        // The original spelling of `converter install`.
        return if arguments.len() == 1 {
            converter_command(&["install".to_owned()])
        } else {
            Err(USAGE.to_owned())
        };
    }
    let verb = arguments.first().ok_or_else(|| USAGE.to_owned())?;
    let input = arguments.get(1).ok_or_else(|| USAGE.to_owned())?;
    let mut out = None;
    let mut target = None;
    let mut title = None;
    let mut section = None;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--out" => {
                out = Some(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| USAGE.to_owned())?
                        .as_str(),
                );
                index += 2;
            }
            "--title" => {
                title = Some(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| USAGE.to_owned())?
                        .as_str(),
                );
                index += 2;
            }
            "--section" => {
                section = Some(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| USAGE.to_owned())?
                        .as_str(),
                );
                index += 2;
            }
            "--sim" => {
                target = Some("");
                index += 1;
            }
            flag if super::is_device_flag(flag) => {
                target = Some(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| USAGE.to_owned())?
                        .as_str(),
                );
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let input_path = Path::new(input);
    let report = prepare_any(input_path, title, section)?;
    match verb.as_str() {
        "prepare" => write_pattern(
            Path::new(out.ok_or_else(|| USAGE.to_owned())?),
            &report.markdown,
            CopyState::Prepared,
        ),
        "preview" => {
            if let Some(directory) = out {
                write_preview(Path::new(directory), &report, input_path)
            } else {
                print_report(&report, input_path, true);
                Ok(())
            }
        }
        "push" => match (target, out) {
            (Some(""), None) => transfer_sim(&report, input_path),
            (Some(host), None) => transfer(&report, input_path, host),
            (None, Some(path)) => {
                write_pattern(Path::new(path), &report.markdown, CopyState::Prepared)?;
                if !report.charts.is_empty() {
                    println!(
                        "Charts travel with a push to the reader or simulator, not into one file."
                    );
                }
                Ok(())
            }
            _ => Err(USAGE.to_owned()),
        },
        _ => Err(USAGE.to_owned()),
    }
}

fn converter_command(arguments: &[String]) -> Result<(), String> {
    match arguments {
        [action] if action == "status" => converter_status(),
        [action] if action == "install" => {
            if converter_status().is_ok() {
                println!("The PDF converter (pdftotext) is already installed.");
                return Ok(());
            }
            let (program, args): (&str, &[&str]) = if cfg!(target_os = "windows")
                && command_exists("winget")
            {
                (
                    "winget",
                    &["install", "--id", "oschwartz10612.Poppler", "-e"],
                )
            } else if command_exists("brew") {
                ("brew", &["install", "poppler"])
            } else if command_exists("apt-get") {
                ("sudo", &["apt-get", "install", "-y", "poppler-utils"])
            } else if command_exists("dnf") {
                ("sudo", &["dnf", "install", "-y", "poppler-utils"])
            } else if command_exists("pacman") {
                ("sudo", &["pacman", "-S", "--needed", "poppler"])
            } else {
                return Err("No supported package manager found. Needles can prepare Markdown and text now, but PDF preparation needs Poppler pdftotext.".to_owned());
            };
            println!(
                "Installing Needles PDF support with {program} {}",
                args.join(" ")
            );
            let status = ProcessCommand::new(program)
                .args(args)
                .status()
                .map_err(|error| format!("could not start {program}: {error}"))?;
            if !status.success() {
                return Err("Poppler installation did not finish successfully".to_owned());
            }
            converter_status()
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn converter_status() -> Result<(), String> {
    // A status query that determined the status succeeded: the answer goes
    // to stdout and the exit code stays 0, whatever the answer is.
    match Command::new("pdftotext").arg("-v").output() {
        Ok(output) if output.status.success() => {
            println!("PDF converter ready: Poppler pdftotext");
        }
        Ok(_) => {
            return Err("Poppler pdftotext is installed but did not run successfully".to_owned());
        }
        Err(_) => {
            println!(
                "Needles PDF support is not installed. Run `kobo needles converter install`, or use a Markdown/text pattern now."
            );
        }
    }
    Ok(())
}

fn command_exists(name: &str) -> bool {
    ProcessCommand::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn select_section(markdown: &[u8], wanted: &str) -> Result<Vec<u8>, String> {
    let text = std::str::from_utf8(markdown).map_err(|_| "pattern is not UTF-8".to_owned())?;
    let wanted = wanted.trim();
    let mut selected = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let heading = line.strip_prefix("## ").map(str::trim);
        if let Some(heading) = heading {
            if in_section {
                break;
            }
            if heading.eq_ignore_ascii_case(wanted) {
                in_section = true;
                selected.push(format!("# {heading}"));
                continue;
            }
        }
        if in_section {
            selected.push(line.to_owned());
        }
    }
    if !in_section {
        let available = text
            .lines()
            .filter_map(|line| line.strip_prefix("## "))
            .take(12)
            .collect::<Vec<_>>();
        return Err(if available.is_empty() {
            "this pattern has no ## section headings to choose from".to_owned()
        } else {
            format!(
                "section {wanted:?} was not found; available: {}",
                available.join(", ")
            )
        });
    }
    Ok(format!("{}\n", selected.join("\n")).into_bytes())
}

struct Report {
    markdown: Vec<u8>,
    source: String,
    pages: usize,
    image_only: Vec<usize>,
    has_images: bool,
    /// The name the reader counts under.
    title: String,
    /// The sections the reader will count by, in the pattern's own words.
    sections: Vec<String>,
    rows: Vec<String>,
    /// The chart names the pattern refers to, in the order it refers to them.
    charts: Vec<String>,
}

fn prepare_any(input: &Path, title: Option<&str>, section: Option<&str>) -> Result<Report, String> {
    let source = if has_extension(input, "pdf") {
        "PDF"
    } else if has_extension(input, "md") {
        "Markdown"
    } else if has_extension(input, "txt") {
        "Plain text"
    } else {
        return Err("Needles accepts a .pdf, .md or .txt pattern file".to_owned());
    };
    let (mut markdown, pages, image_only, has_images) = if source == "PDF" {
        prepare_pdf(input, title)?
    } else {
        let bytes = read_pattern(input)?;
        let body = String::from_utf8(bytes).map_err(|_| "pattern is not UTF-8".to_owned())?;
        (normalize_text(input, &body, title), 1, Vec::new(), false)
    };
    if let Some(wanted) = section {
        markdown = select_section(&markdown, wanted)?;
    }
    let rows = {
        let text = String::from_utf8_lossy(&markdown);
        text.lines()
            .filter(|line| {
                let l = line.to_ascii_lowercase();
                l.starts_with("row ")
                    || l.starts_with("rows ")
                    || l.starts_with("round ")
                    || l.starts_with("rounds ")
                    || l.starts_with("rnd ")
            })
            .map(str::to_owned)
            .take(200)
            .collect()
    };
    if markdown.len() > MAX_PATTERN {
        return Err("the prepared pattern is too large for this reader".to_owned());
    }
    let (title, sections, charts) = outline(&markdown, input);
    Ok(Report {
        markdown: std::mem::take(&mut markdown),
        source: source.to_owned(),
        pages,
        image_only,
        has_images,
        title,
        sections,
        rows,
        charts,
    })
}

/// The title, the sections the reader counts by, and the chart names the
/// pattern refers to, read with the same Markdown parser the reader uses,
/// so what the preview says is what the counter does.
fn outline(markdown: &[u8], input: &Path) -> (String, Vec<String>, Vec<String>) {
    let text = String::from_utf8_lossy(markdown);
    let document = kobo_doc::markdown::parse(&text);
    let mut headings = Vec::new();
    let mut charts: Vec<String> = Vec::new();
    for block in &document.blocks {
        match block {
            kobo_doc::Block::Heading { level, text } => headings.push((*level, text.clone())),
            kobo_doc::Block::Picture { name, .. } => {
                if !charts.contains(name) {
                    charts.push(name.clone());
                }
            }
            _ => {}
        }
    }
    let title = headings
        .iter()
        .find(|(level, _)| *level == 1)
        .map_or_else(|| title_from_stem(input), |(_, text)| text.clone());
    // The reader's rule: second-level headings when the pattern has that
    // much structure, a flat pattern's own headings past its title otherwise.
    let sections: Vec<String> = if headings.iter().any(|(level, _)| *level == 2) {
        headings
            .iter()
            .filter(|(level, _)| *level == 2)
            .map(|(_, text)| text.clone())
            .collect()
    } else {
        let mut past_title: Vec<String> = headings
            .iter()
            .filter(|(level, _)| *level == 1)
            .map(|(_, text)| text.clone())
            .collect();
        if let Some(first) = past_title.first().cloned() {
            if Some(&first) == headings.first().map(|(_, text)| text) {
                past_title.remove(0);
            }
        }
        if past_title.len() > 1 {
            past_title
        } else {
            Vec::new()
        }
    };
    (title, sections, charts)
}

fn title_from_stem(input: &Path) -> String {
    input
        .file_stem()
        .and_then(OsStr::to_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Pattern")
        .to_owned()
}

/// The chart file sitting next to the pattern, when one was exported with it.
fn chart_beside(input: &Path, name: &str) -> Option<PathBuf> {
    if !kobo_protocol::is_valid_key(name) {
        return None;
    }
    let path = input.parent().unwrap_or_else(|| Path::new(".")).join(name);
    path.is_file().then_some(path)
}

/// What the reader will do with the pattern, said before it goes anywhere.
fn print_report(report: &Report, input: &Path, with_body: bool) {
    println!("Pattern: {}", report.title);
    if report.sections.is_empty() {
        println!(
            "Sections: none found - the reader counts by Body, Sleeve and Finishing until the pattern names its own with ## headings"
        );
    } else {
        println!("Sections: {}", report.sections.join(", "));
    }
    for chart in &report.charts {
        if chart_beside(input, chart).is_some() {
            println!("Chart: {chart}");
        } else {
            println!(
                "Chart: {chart} - not found next to the pattern; on the reader it shows as its caption"
            );
        }
    }
    for page in &report.image_only {
        println!(
            "Note: page {page} has no extractable text, so it is probably a chart or a scan; export it as a PNG and keep it next to the pattern when you push"
        );
    }
    if with_body {
        let body = String::from_utf8_lossy(&report.markdown);
        println!("---");
        for line in body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(PREVIEW_LINES)
        {
            println!("  {line}");
        }
    }
}
fn normalize_text(input: &Path, body: &str, title: Option<&str>) -> Vec<u8> {
    let fallback = input
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Pattern");
    let title = title.unwrap_or(fallback);
    let trimmed = body.replace('\0', " ").trim().to_owned();
    if trimmed.starts_with("# ") {
        format!("{trimmed}\n").into_bytes()
    } else {
        format!("# {title}\n\n{trimmed}\n").into_bytes()
    }
}
fn prepare_pdf(
    input: &Path,
    title: Option<&str>,
) -> Result<(Vec<u8>, usize, Vec<usize>, bool), String> {
    if Command::new("pdftotext").arg("-v").output().is_err() {
        return Err("Poppler pdftotext is not installed. Install the poppler package, then rerun `kobo needles converter status`.".to_owned());
    }
    let markdown = prepare(input)?;
    let text = String::from_utf8_lossy(&markdown);
    let page_text = text.split('\u{c}').collect::<Vec<_>>();
    let image_only = page_text
        .iter()
        .enumerate()
        .filter_map(|(i, page)| (page.trim().chars().count() < 20).then_some(i + 1))
        .collect();
    let has_images = Command::new("pdfimages")
        .args(["-list", input.to_str().unwrap_or("")])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().nth(2).is_some());
    let body = text.lines().skip(1).collect::<Vec<_>>().join("\n");
    Ok((
        normalize_text(input, &body, title),
        page_text.len(),
        image_only,
        has_images,
    ))
}
#[derive(Clone, Copy)]
enum CopyState {
    Prepared,
    Simulator,
}

fn completion(state: CopyState, path: &Path) -> String {
    match state {
        CopyState::Prepared => format!(
            "Prepared locally (not sent): {}\nThis copy is ready to review without a reader.",
            path.display()
        ),
        CopyState::Simulator => format!(
            "Sent to simulator: {}\nAvailable offline in Needles.",
            path.display()
        ),
    }
}

fn write_pattern(path: &Path, bytes: &[u8], state: CopyState) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    println!("{}", completion(state, path));
    Ok(())
}
/// Lays the pattern, and the charts that sit next to it, onto the
/// simulator's shelf the way a device transfer would place them.
fn transfer_sim(report: &Report, input: &Path) -> Result<(), String> {
    let root = kobo_sim::simulated_data_root("needles");
    std::fs::create_dir_all(&root).map_err(|e| format!("create Needles shelf: {e}"))?;
    write_blob(&root, BLOB, &report.markdown)?;
    let mut sent = 0;
    for chart in &report.charts {
        let Some(path) = chart_beside(input, chart) else {
            continue;
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read chart {}: {error}", path.display()))?;
        write_blob(&root, chart, &bytes)?;
        sent += 1;
    }
    if sent > 0 {
        println!(
            "Sent to simulator: {} and {sent} chart(s)\nAvailable offline in Needles.",
            root.join(BLOB).display()
        );
    } else {
        println!("{}", completion(CopyState::Simulator, &root.join(BLOB)));
    }
    Ok(())
}

fn write_blob(root: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let partial = root.join(format!(".{name}.writing"));
    std::fs::write(&partial, bytes).map_err(|error| format!("write {name}: {error}"))?;
    std::fs::rename(&partial, root.join(name)).map_err(|error| format!("publish {name}: {error}"))
}
fn write_preview(path: &Path, report: &Report, input: &Path) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "preview directory {} already exists",
            path.display()
        ));
    }
    std::fs::create_dir(path).map_err(|e| format!("create preview: {e}"))?;
    let mut html = String::from(
        "<!doctype html><html><meta charset=utf-8><meta name=viewport content='width=device-width'><style>body{font:17px/1.5 system-ui;max-width:760px;margin:auto;padding:24px;white-space:pre-wrap}</style><h1>Needles pattern preview</h1>",
    );
    write!(
        html,
        "<p>Source: {} · {} page(s)</p>",
        report.source, report.pages
    )
    .unwrap();
    if report.has_images {
        html.push_str("<p>Images/charts detected: compare the extracted instructions with the source before sending.</p>");
    }
    if !report.image_only.is_empty() {
        write!(
            html,
            "<p>Pages with little or no extractable text: {:?}</p>",
            report.image_only
        )
        .unwrap();
    }
    for chart in &report.charts {
        let name = chart.replace('&', "&amp;").replace('<', "&lt;");
        if chart_beside(input, chart).is_some() {
            write!(html, "<p>Chart: {name} (travels with the push)</p>").unwrap();
        } else {
            write!(
                html,
                "<p>Chart: {name} (not found next to the pattern; on the reader it shows as its caption)</p>"
            )
            .unwrap();
        }
    }
    write!(
        html,
        "<p>Sections: {}</p><p>Rows found: {}</p><hr><pre>{}</pre>",
        report.sections.join(" · "),
        report.rows.len(),
        String::from_utf8_lossy(&report.markdown)
            .replace('&', "&amp;")
            .replace('<', "&lt;")
    )
    .unwrap();
    std::fs::write(path.join("index.html"), html).map_err(|e| format!("write preview: {e}"))?;
    std::fs::write(path.join("pattern.md"), &report.markdown)
        .map_err(|e| format!("write preview pattern: {e}"))?;
    println!(
        "Prepared locally (not sent): {}\nThis preview is ready to review without a reader.",
        path.join("index.html").display()
    );
    Ok(())
}

fn prepare(input: &Path) -> Result<Vec<u8>, String> {
    if !has_extension(input, "pdf") {
        return Err("Needles preparation accepts a .pdf file".to_owned());
    }
    let metadata = std::fs::metadata(input)
        .map_err(|error| format!("could not read {}: {error}", input.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", input.display()));
    }
    if usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_PDF {
        return Err(format!(
            "{} is larger than {} MB; split the pattern before preparing it",
            input.display(),
            MAX_PDF / (1024 * 1024)
        ));
    }

    let mut child = Command::new("pdftotext")
        .arg("-layout")
        .arg(input)
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            format!(
                "could not start pdftotext ({error}); install Poppler to prepare this user-owned PDF"
            )
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("pdftotext did not provide extracted text")?;
    let text = read_limited(&mut stdout, MAX_PATTERN)?;
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for pdftotext: {error}"))?;
    if !status.success() {
        return Err(
            "pdftotext could not extract this PDF; it may be encrypted or malformed".to_owned(),
        );
    }
    if text.len() > MAX_PATTERN {
        return Err(
            "the extracted pattern is too large for this reader; split it before preparing"
                .to_owned(),
        );
    }
    let text = String::from_utf8(text)
        .map_err(|_| "pdftotext produced non-text output for this PDF".to_owned())?;
    let body = text.replace('\0', " ").trim().to_owned();
    if body.is_empty() {
        return Err(
            "this PDF has no extractable text. Scanned pages and charts need image support, which Needles v1 does not yet transfer."
                .to_owned(),
        );
    }
    let title = input
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Pattern");
    let markdown = format!("# {title}\n\n{body}\n");
    if markdown.len() > MAX_PATTERN {
        return Err(
            "the extracted pattern is too large for this reader; split it before preparing"
                .to_owned(),
        );
    }
    Ok(markdown.into_bytes())
}

fn read_pattern(input: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(input)
        .map_err(|error| format!("could not read {}: {error}", input.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", input.display()));
    }
    if usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_PATTERN {
        return Err(
            "the prepared pattern is too large for this reader; split it before transfer"
                .to_owned(),
        );
    }
    let bytes = std::fs::read(input)
        .map_err(|error| format!("could not read {}: {error}", input.display()))?;
    if std::str::from_utf8(&bytes).is_err() {
        return Err("the prepared pattern must be UTF-8 Markdown or plain text".to_owned());
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err("the prepared pattern is empty".to_owned());
    }
    Ok(bytes)
}

/// Drains all of `reader` so the PDF process can exit, keeping only a bounded
/// prefix in memory. Keeping a pipe unread after its ceiling would deadlock a
/// converter that is still trying to write its remaining pages.
fn read_limited(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("could not read extracted PDF text: {error}"))?;
        if read == 0 {
            return Ok(output);
        }
        let remaining = limit.saturating_add(1).saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

fn transfer(report: &Report, input: &Path, host: &str) -> Result<(), String> {
    let mut script =
        "set -e\nroot=/mnt/onboard/.adds/cobalt/data/needles\nmkdir -p \"$root\"\n".to_owned();
    script.push_str(&blob_script(BLOB, &report.markdown));
    let mut sent = 0;
    for chart in &report.charts {
        let Some(path) = chart_beside(input, chart) else {
            continue;
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read chart {}: {error}", path.display()))?;
        script.push_str(&blob_script(chart, &bytes));
        sent += 1;
    }
    script.push_str("sync\nprintf 'Sent to reader: Needles pattern'\n");
    if sent > 0 {
        script.push_str(&format!(" && printf ' and {sent} chart(s)'"));
    }
    script.push_str(" && printf '\\nAvailable offline in Needles.\\n'\n");
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the Needles pattern transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn blob_script(name: &str, bytes: &[u8]) -> String {
    format!(
        "partial=\"$root/.{name}.writing\"\n\
         base64 -d > \"$partial\" <<'KOBO_NEEDLES_BLOB'\n\
         {}\n\
         KOBO_NEEDLES_BLOB\n\
         chmod 600 \"$partial\"\n\
         mv -f \"$partial\" \"$root/{name}\"\n",
        super::base64_encode(bytes)
    )
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

#[cfg(test)]
fn has_text_extension(path: &Path) -> bool {
    has_extension(path, "md") || has_extension(path, "txt")
}

#[cfg(test)]
mod tests {
    use super::{
        command, completion, has_extension, has_text_extension, prepare_any, read_limited,
        read_pattern, select_section, transfer_sim, CopyState, BLOB,
    };
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    #[test]
    fn completion_keeps_prepared_and_sent_states_distinct() {
        let path = Path::new("pattern.md");
        let prepared = completion(CopyState::Prepared, path);
        assert!(prepared.contains("Prepared locally (not sent)"));
        assert!(prepared.contains("without a reader"));
        let sent = completion(CopyState::Simulator, path);
        assert!(sent.contains("Sent to simulator"));
        assert!(sent.contains("Available offline"));
        assert!(!sent.contains("not sent"));
    }

    #[test]
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
    }

    #[test]
    fn accepts_only_declared_input_extensions() {
        assert!(has_extension(Path::new("Pattern.PDF"), "pdf"));
        assert!(has_text_extension(Path::new("Pattern.md")));
        assert!(has_text_extension(Path::new("Pattern.txt")));
        assert!(!has_text_extension(Path::new("Pattern.pdf")));
        assert_eq!(BLOB, "pattern.md");
    }

    #[test]
    fn selects_one_named_section_and_its_rows() {
        let pattern =
            b"# Book\n\n## Scarf\n\nRow 1: Knit.\nRow 2: Purl.\n\n## Hat\n\nRound 1: Knit.\n";
        assert_eq!(
            String::from_utf8(select_section(pattern, "scarf").expect("section")).unwrap(),
            "# Scarf\n\nRow 1: Knit.\nRow 2: Purl.\n\n"
        );
        assert!(select_section(pattern, "Socks").is_err());
    }

    #[test]
    fn rejects_non_utf8_prepared_patterns() {
        let path = std::env::temp_dir().join(format!("needles-invalid-{}", std::process::id()));
        std::fs::write(&path, [0xff]).expect("fixture");
        assert!(read_pattern(&path).is_err());
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn drains_but_never_keeps_more_than_the_pattern_ceiling() {
        let mut source = Cursor::new(vec![b'x'; 17]);
        assert_eq!(read_limited(&mut source, 4).expect("read"), vec![b'x'; 5]);
    }

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("needles-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let path = directory.join(name);
        std::fs::write(&path, bytes).expect("fixture");
        path
    }

    #[test]
    fn a_markdown_pattern_keeps_its_own_title_and_sections() {
        let path = write_temp(
            "needles-socks.md",
            b"# Winter socks\n\n## Cuff\n\nWork 12 rows.\n\n## Leg\n\nWork 30 rows.\n",
        );
        let report = prepare_any(&path, None, None).expect("prepared");
        assert_eq!(report.title, "Winter socks");
        assert_eq!(report.sections, ["Cuff", "Leg"]);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_plain_text_pattern_gets_a_title_to_count_under() {
        let path = write_temp(
            "needles-dishcloth.txt",
            b"Cast on 40 stitches.\nKnit every row.\n",
        );
        let report = prepare_any(&path, None, None).expect("prepared");
        assert_eq!(report.title, "needles-dishcloth");
        let markdown = String::from_utf8(report.markdown).expect("utf8");
        assert!(markdown.starts_with("# needles-dishcloth\n\nCast on 40 stitches."));
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn an_explicit_title_names_a_headingless_pattern() {
        let path = write_temp(
            "needles-untitled.txt",
            b"Cast on 40 stitches.\nKnit every row.\n",
        );
        let report = prepare_any(&path, Some("Winter socks"), None).expect("prepared");
        assert_eq!(report.title, "Winter socks");
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn charts_are_named_in_order_without_repeats() {
        let path = write_temp(
            "needles-charted.md",
            b"# Socks\n\n![Main chart](chart-main.png)\n\n## Leg\n\n![Main chart again](chart-main.png)\n\n![Web chart](https://shared.example/chart.png)\n",
        );
        let report = prepare_any(&path, None, None).expect("prepared");
        assert_eq!(
            report.charts,
            ["chart-main.png", "https://shared.example/chart.png"]
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_flat_pattern_counts_by_its_headings_past_the_title() {
        let path = write_temp("needles-flat.md", b"# Dishcloth\n\n# Body\n\n# Edging\n");
        let report = prepare_any(&path, None, None).expect("prepared");
        assert_eq!(report.sections, ["Body", "Edging"]);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_title_alone_is_not_a_section_list() {
        let path = write_temp("needles-bare.md", b"# Just a title\n\nPlain rows.\n");
        let report = prepare_any(&path, None, None).expect("prepared");
        assert!(report.sections.is_empty());
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_push_to_the_simulator_lays_the_pattern_and_its_charts_on_the_shelf() {
        let path = write_temp(
            "needles-sim-chart.md",
            b"# Simmed\n\n## Cuff\n\n![Cuff chart](chart-cuff.png)\n\nRows.\n",
        );
        let chart = path.parent().expect("parent").join("chart-cuff.png");
        std::fs::write(&chart, b"png-bytes").expect("chart fixture");
        let report = prepare_any(&path, None, None).expect("prepared");
        transfer_sim(&report, &path).expect("transfer");
        let root = kobo_sim::simulated_data_root("needles");
        let written = std::fs::read(root.join(BLOB)).expect("shelf blob");
        assert_eq!(written, report.markdown);
        assert_eq!(
            std::fs::read(root.join("chart-cuff.png")).expect("shelf chart"),
            b"png-bytes"
        );
        std::fs::remove_file(path).expect("cleanup");
        std::fs::remove_file(chart).expect("cleanup");
        std::fs::remove_file(root.join(BLOB)).expect("cleanup");
        std::fs::remove_file(root.join("chart-cuff.png")).expect("cleanup");
    }

    #[test]
    fn setup_is_the_install_alias() {
        // No package manager runs in the test: an installed converter answers.
        assert!(command(&["setup".into(), "extra".into()]).is_err());
    }
}
