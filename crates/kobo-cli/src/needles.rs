//! Owner-attended preparation and transfer for Needles pattern documents.
//!
//! PDF parsing belongs on the host: this keeps the reader application small,
//! lets the shared book reader handle reflow, and never sends credentials here.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::process::{Command, Stdio};

const MAX_PDF: usize = 32 * 1024 * 1024;
const MAX_PATTERN: usize = 4 * 1024 * 1024;
const BLOB: &str = "pattern.md";
const USAGE: &str = "usage: kobo needles converter status|install\n\
                     \x20      kobo needles prepare PATTERN.(pdf|md|txt) --out PATTERN.md [--title TITLE] [--section HEADING]\n\
                     \x20      kobo needles preview PATTERN.(pdf|md|txt) --out DIRECTORY [--title TITLE] [--section HEADING]\n\
                     \x20      kobo needles push PATTERN.(pdf|md|txt) (--sim | --device IP | --out FILE) [--title TITLE] [--section HEADING]";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    if arguments.first().map(String::as_str) == Some("converter") {
        return converter_command(&arguments[1..]);
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
    let report = prepare_any(Path::new(input), title, section)?;
    match verb.as_str() {
        "prepare" => write_pattern(
            Path::new(out.ok_or_else(|| USAGE.to_owned())?),
            &report.markdown,
            CopyState::Prepared,
        ),
        "preview" => write_preview(Path::new(out.ok_or_else(|| USAGE.to_owned())?), &report),
        "push" => match (target, out) {
            (Some(""), None) => publish_local(&report.markdown),
            (Some(host), None) => transfer(&report.markdown, host),
            (None, Some(path)) => {
                write_pattern(Path::new(path), &report.markdown, CopyState::Prepared)
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
                return Ok(());
            }
            let (program, args): (&str, &[&str]) = if command_exists("brew") {
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
    let output = Command::new("pdftotext").arg("-v").output().map_err(|_| {
        "Needles PDF support is not installed. Run `kobo needles converter install`, or use a Markdown/text pattern now.".to_owned()
    })?;
    if !output.status.success() {
        return Err("Poppler pdftotext is installed but did not run successfully".to_owned());
    }
    println!("PDF converter ready: Poppler pdftotext");
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
    sections: Vec<String>,
    rows: Vec<String>,
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
    let text = String::from_utf8_lossy(&markdown);
    let sections = text
        .lines()
        .filter_map(|line| line.strip_prefix("## ").map(str::to_owned))
        .take(40)
        .collect();
    let rows = text
        .lines()
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
        .collect();
    if markdown.len() > MAX_PATTERN {
        return Err("the prepared pattern is too large for this reader".to_owned());
    }
    Ok(Report {
        markdown: std::mem::take(&mut markdown),
        source: source.to_owned(),
        pages,
        image_only,
        has_images,
        sections,
        rows,
    })
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
fn publish_local(bytes: &[u8]) -> Result<(), String> {
    let root = kobo_sim::simulated_data_root("needles");
    std::fs::create_dir_all(&root).map_err(|e| format!("create Needles shelf: {e}"))?;
    write_pattern(&root.join(BLOB), bytes, CopyState::Simulator)
}
fn write_preview(path: &Path, report: &Report) -> Result<(), String> {
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

fn transfer(bytes: &[u8], host: &str) -> Result<(), String> {
    let encoded = super::base64_encode(bytes);
    let script = format!(
        "set -e\n\
         root=/mnt/onboard/.adds/cobalt/data/needles\n\
         mkdir -p \"$root\"\n\
         partial=\"$root/.{BLOB}.writing\"\n\
         base64 -d > \"$partial\" <<'KOBO_NEEDLES_PATTERN'\n\
         {encoded}\n\
         KOBO_NEEDLES_PATTERN\n\
         chmod 600 \"$partial\"\n\
         mv -f \"$partial\" \"$root/{BLOB}\"\n\
         sync\n\
         printf 'Sent to reader: Needles pattern\\nAvailable offline in Needles.\\n'\n"
    );
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
        completion, has_extension, has_text_extension, read_limited, read_pattern, select_section,
        CopyState, BLOB,
    };
    use std::io::Cursor;
    use std::path::Path;

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
}
