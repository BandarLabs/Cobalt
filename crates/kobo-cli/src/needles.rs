//! Owner-attended preparation and transfer for Needles pattern documents.
//!
//! PDF parsing belongs on the host: this keeps the reader application small,
//! lets the shared book reader handle reflow, and never sends credentials here.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_PDF: usize = 32 * 1024 * 1024;
const MAX_PATTERN: usize = 4 * 1024 * 1024;
const BLOB: &str = "pattern.md";
/// How much of the extracted pattern a preview shows against its source.
const PREVIEW_LINES: usize = 12;
const USAGE: &str = "usage: kobo needles prepare PATTERN.(pdf|md|txt) --out PATTERN.md [--title NAME]\n\
                     \x20      kobo needles preview PATTERN.(pdf|md|txt) [--title NAME]\n\
                     \x20      kobo needles push PATTERN.(pdf|md|txt) (--sim | --device IP | --out PATH) [--title NAME]\n\
                     \x20      kobo needles setup";

/// A pattern ready to leave: the document itself, what the reader will make
/// of it, and what is worth knowing before it goes.
struct Prepared {
    markdown: Vec<u8>,
    title: String,
    /// The sections the reader will count by, in the pattern's own words.
    sections: Vec<String>,
    /// The chart names the pattern refers to, in the order it refers to them.
    charts: Vec<String>,
    /// Image-only pages and other things to know before the pattern leaves.
    notes: Vec<String>,
}

/// Where a push is going.
enum Target {
    Sim,
    Device(String),
    Out(PathBuf),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("prepare") => prepare_command(&arguments[1..]),
        Some("preview") => preview_command(&arguments[1..]),
        Some("push") => push_command(&arguments[1..]),
        Some("setup") => setup(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn prepare_command(arguments: &[String]) -> Result<(), String> {
    let (rest, title) = take_title(arguments);
    match rest.as_slice() {
        [input, flag, output] if flag == "--out" => {
            let prepared = prepare(Path::new(input), title.as_deref())?;
            print_report(&prepared, Path::new(input), true);
            std::fs::write(output, &prepared.markdown)
                .map_err(|error| format!("could not write {output}: {error}"))?;
            println!("Prepared Needles pattern: {output}");
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn preview_command(arguments: &[String]) -> Result<(), String> {
    let (rest, title) = take_title(arguments);
    match rest.as_slice() {
        [input] => {
            let prepared = prepare(Path::new(input), title.as_deref())?;
            print_report(&prepared, Path::new(input), true);
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn push_command(arguments: &[String]) -> Result<(), String> {
    let (rest, title) = take_title(arguments);
    let Some((input, target)) = rest.split_first() else {
        return Err(USAGE.to_owned());
    };
    let target = parse_target(target)?;
    let input = Path::new(input);
    let prepared = prepare(input, title.as_deref())?;
    print_report(&prepared, input, false);
    match target {
        Target::Sim => transfer_sim(&prepared, input),
        Target::Device(host) => transfer(&prepared, input, &host),
        Target::Out(path) => {
            std::fs::write(&path, &prepared.markdown)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            if !prepared.charts.is_empty() {
                println!(
                    "Charts travel with a push to the reader or simulator, not into one file."
                );
            }
            println!("Prepared Needles pattern: {}", path.display());
            Ok(())
        }
    }
}

/// What the reader will do with the pattern, said before it goes anywhere.
fn print_report(prepared: &Prepared, input: &Path, with_body: bool) {
    println!("Pattern: {}", prepared.title);
    if prepared.sections.is_empty() {
        println!("Sections: none found - the reader counts by Body, Sleeve and Finishing until the pattern names its own with ## headings");
    } else {
        println!("Sections: {}", prepared.sections.join(", "));
    }
    for chart in &prepared.charts {
        if chart_beside(input, chart).is_some() {
            println!("Chart: {chart}");
        } else {
            println!("Chart: {chart} - not found next to the pattern; on the reader it shows as its caption");
        }
    }
    for note in &prepared.notes {
        println!("Note: {note}");
    }
    if with_body {
        let body = String::from_utf8_lossy(&prepared.markdown);
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

fn prepare(input: &Path, title: Option<&str>) -> Result<Prepared, String> {
    if has_extension(input, "pdf") {
        prepare_pdf(input, title)
    } else if has_text_extension(input) {
        prepare_text(input, title)
    } else {
        Err("Needles accepts a .pdf, .md or .txt pattern file".to_owned())
    }
}

fn prepare_text(input: &Path, title: Option<&str>) -> Result<Prepared, String> {
    let bytes = read_pattern(input)?;
    let text = String::from_utf8_lossy(&bytes);
    let found = first_heading(&text);
    let title = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .or(found)
        .unwrap_or_else(|| title_from_stem(input));
    // A document with no heading of its own gets the chosen one, so the
    // reader always has a name to count under.
    let markdown = if text.trim_start().starts_with('#') {
        text.trim().to_owned()
    } else {
        format!("# {title}\n\n{}", text.trim())
    };
    finish(title, markdown)
}

fn prepare_pdf(input: &Path, title: Option<&str>) -> Result<Prepared, String> {
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

    let converter = converter()?;
    let mut child = Command::new(converter)
        .arg("-layout")
        .arg(input)
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start the PDF converter: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("the PDF converter did not provide extracted text")?;
    let text = read_limited(&mut stdout, MAX_PATTERN)?;
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for the PDF converter: {error}"))?;
    if !status.success() {
        return Err(
            "the PDF converter could not extract this PDF; it may be encrypted or malformed"
                .to_owned(),
        );
    }
    if text.len() > MAX_PATTERN {
        return Err(
            "the extracted pattern is too large for this reader; split it before preparing"
                .to_owned(),
        );
    }
    let text = String::from_utf8(text)
        .map_err(|_| "the PDF converter produced non-text output for this PDF".to_owned())?;
    // pdftotext parts pages with a form feed; a page with nothing on it but
    // ink is a chart or a scan, and saying so is kinder than a silent gap.
    let pages: Vec<&str> = text.split('\u{c}').collect();
    let mut notes = Vec::new();
    let mut kept = Vec::new();
    for (index, part) in pages.iter().enumerate() {
        if part.chars().any(char::is_alphanumeric) {
            kept.push(*part);
        } else if index + 1 < pages.len() || !part.trim().is_empty() {
            // The piece after the last form feed is a page break's shadow,
            // not a page; everything else that prints nothing had ink.
            notes.push(format!(
                "page {} has no extractable text, so it is probably a chart or a scan; export it as a PNG and keep it next to the pattern when you push",
                index + 1
            ));
        }
    }
    let body = kept.join("\n").replace('\0', " ").trim().to_owned();
    if body.is_empty() {
        return Err(
            "this PDF has no extractable text. Scanned pages and charts are images: export them as PNGs and keep them next to the pattern when you push."
                .to_owned(),
        );
    }
    let title = match title.map(str::trim).filter(|title| !title.is_empty()) {
        Some(title) => title.to_owned(),
        None => title_from_stem(input),
    };
    let markdown = format!("# {title}\n\n{body}\n");
    let mut prepared = finish(title, markdown)?;
    prepared.notes = notes;
    Ok(prepared)
}

/// The outline and the charts, read with the same Markdown parser the reader
/// uses, so what the preview says is what the counter does.
fn finish(title: String, markdown: String) -> Result<Prepared, String> {
    if markdown.len() > MAX_PATTERN {
        return Err(
            "the prepared pattern is too large for this reader; split it before transfer"
                .to_owned(),
        );
    }
    let document = kobo_doc::markdown::parse(&markdown);
    let mut headings = Vec::new();
    let mut charts = Vec::new();
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
    // The reader's rule: second-level headings when the pattern has that much
    // structure, a flat pattern's own headings past its title otherwise.
    let sections = if headings.iter().any(|(level, _)| *level == 2) {
        headings
            .iter()
            .filter(|(level, _)| *level == 2)
            .map(|(_, text)| text.clone())
            .collect()
    } else {
        let flat: Vec<String> = headings
            .iter()
            .filter(|(level, _)| *level == 1)
            .map(|(_, text)| text.clone())
            .collect();
        let mut past_title = flat;
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
    Ok(Prepared {
        markdown: markdown.into_bytes(),
        title,
        sections,
        charts,
        notes: Vec::new(),
    })
}

fn first_heading(text: &str) -> Option<String> {
    let document = kobo_doc::markdown::parse(text);
    document.blocks.iter().find_map(|block| match block {
        kobo_doc::Block::Heading { level: 1, text } => Some(text.clone()),
        _ => None,
    })
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

/// Pulls `--title NAME` off an argument list, leaving the rest in place.
fn take_title(arguments: &[String]) -> (Vec<String>, Option<String>) {
    let mut rest = Vec::with_capacity(arguments.len());
    let mut title = None;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--title" {
            if let Some(name) = arguments.get(index + 1) {
                title = Some(name.clone());
                index += 2;
                continue;
            }
        }
        rest.push(arguments[index].clone());
        index += 1;
    }
    (rest, title)
}

fn parse_target(arguments: &[String]) -> Result<Target, String> {
    match arguments {
        [flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok(Target::Device(host.clone()))
        }
        [flag] if flag == "--sim" => Ok(Target::Sim),
        [flag, path] if flag == "--out" => Ok(Target::Out(PathBuf::from(path))),
        _ => Err(USAGE.to_owned()),
    }
}

/// The PDF converter, found rather than assumed: what is on PATH, then a copy
/// Cobalt keeps. When neither answers, `kobo needles setup` is the way out.
fn converter() -> Result<PathBuf, String> {
    if command_answers("pdftotext") {
        return Ok(PathBuf::from("pdftotext"));
    }
    let managed = tools_dir().join(if cfg!(windows) {
        "pdftotext.exe"
    } else {
        "pdftotext"
    });
    if command_answers(&managed.to_string_lossy()) {
        return Ok(managed);
    }
    Err("no PDF converter (pdftotext) found. Run `kobo needles setup` to install one.".to_owned())
}

fn command_answers(name: &str) -> bool {
    Command::new(name)
        .arg("-v")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn tools_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".cobalt").join("tools")
}

/// Installs the PDF converter with whatever this computer already trusts for
/// installing things, so preparing a PDF never means reading up on toolchains.
fn setup(arguments: &[String]) -> Result<(), String> {
    if !arguments.is_empty() {
        return Err(USAGE.to_owned());
    }
    if command_answers("pdftotext") {
        println!("The PDF converter (pdftotext) is already installed.");
        return Ok(());
    }
    let install: Option<(&str, Vec<&str>)> = if cfg!(target_os = "macos") {
        Some(("brew", vec!["install", "poppler"]))
    } else if cfg!(target_os = "windows") {
        Some((
            "winget",
            vec!["install", "--id", "oschwartz10612.Poppler", "-e"],
        ))
    } else if command_answers("apt-get") {
        Some(("sudo", vec!["apt-get", "install", "-y", "poppler-utils"]))
    } else if command_answers("dnf") {
        Some(("sudo", vec!["dnf", "install", "-y", "poppler-utils"]))
    } else if command_answers("pacman") {
        Some(("sudo", vec!["pacman", "-S", "--noconfirm", "poppler"]))
    } else {
        None
    };
    let Some((program, args)) = install else {
        return Err(
            "no known package manager answered; install Poppler (pdftotext) with whichever this computer uses"
                .to_owned(),
        );
    };
    println!("Installing the PDF converter: {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("could not start {program}: {error}"))?;
    if !status.success() {
        return Err(format!("{program} did not finish the installation"));
    }
    if command_answers("pdftotext") {
        println!("The PDF converter (pdftotext) is installed.");
        Ok(())
    } else {
        Err("the installation finished but pdftotext still does not answer; a new terminal may be needed".to_owned())
    }
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

/// Writes the pattern, and the charts that sit next to it, onto the
/// simulator's shelf the way a device transfer would place them.
fn transfer_sim(prepared: &Prepared, input: &Path) -> Result<(), String> {
    let root = kobo_sim::simulated_data_root("needles");
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("create the simulator Needles shelf: {error}"))?;
    write_blob(&root, BLOB, &prepared.markdown)?;
    let mut sent = 0;
    for chart in &prepared.charts {
        let Some(path) = chart_beside(input, chart) else {
            continue;
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read chart {}: {error}", path.display()))?;
        write_blob(&root, chart, &bytes)?;
        sent += 1;
    }
    if sent > 0 {
        println!("Transferred Needles pattern and {sent} chart(s) to the simulator.");
    } else {
        println!("Transferred Needles pattern to the simulator.");
    }
    Ok(())
}

fn write_blob(root: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let partial = root.join(format!(".{name}.writing"));
    std::fs::write(&partial, bytes).map_err(|error| format!("write {name}: {error}"))?;
    std::fs::rename(&partial, root.join(name)).map_err(|error| format!("publish {name}: {error}"))
}

fn transfer(prepared: &Prepared, input: &Path, host: &str) -> Result<(), String> {
    let mut script =
        "set -e\nroot=/mnt/onboard/.adds/cobalt/data/needles\nmkdir -p \"$root\"\n".to_owned();
    script.push_str(&blob_script(BLOB, &prepared.markdown));
    let mut sent = 0;
    for chart in &prepared.charts {
        let Some(path) = chart_beside(input, chart) else {
            continue;
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read chart {}: {error}", path.display()))?;
        script.push_str(&blob_script(chart, &bytes));
        sent += 1;
    }
    script.push_str("sync\nprintf 'Transferred Needles pattern'\n");
    if sent > 0 {
        script.push_str(&format!(" && printf ' and {sent} chart(s)'"));
    }
    script.push_str(" && printf '\\n'\n");
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

fn has_text_extension(path: &Path) -> bool {
    has_extension(path, "md") || has_extension(path, "txt")
}

#[cfg(test)]
mod tests {
    use super::{
        command, has_extension, has_text_extension, prepare, read_limited, read_pattern,
        take_title, BLOB,
    };
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    #[test]
    fn help_succeeds() {
        command(&["--help".into()]).expect("help");
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
        let prepared = prepare(&path, None).expect("prepared");
        assert_eq!(prepared.title, "Winter socks");
        assert_eq!(prepared.sections, ["Cuff", "Leg"]);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_plain_text_pattern_gets_a_title_to_count_under() {
        let path = write_temp(
            "needles-dishcloth.txt",
            b"Cast on 40 stitches.\nKnit every row.\n",
        );
        let prepared = prepare(&path, None).expect("prepared");
        assert_eq!(prepared.title, "needles-dishcloth");
        let markdown = String::from_utf8(prepared.markdown).expect("utf8");
        assert!(markdown.starts_with("# needles-dishcloth\n\nCast on 40 stitches."));
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn an_explicit_title_wins_over_the_file_and_the_document() {
        let path = write_temp(
            "needles-socks-title.md",
            b"# Wrong name\n\n## Cuff\n\nRows.\n",
        );
        let prepared = prepare(&path, Some("Winter socks")).expect("prepared");
        assert_eq!(prepared.title, "Winter socks");
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn charts_are_named_in_order_without_repeats() {
        let path = write_temp(
            "needles-charted.md",
            b"# Socks\n\n![Main chart](chart-main.png)\n\n## Leg\n\n![Main chart again](chart-main.png)\n\n![Web chart](https://shared.example/chart.png)\n",
        );
        let prepared = prepare(&path, None).expect("prepared");
        assert_eq!(
            prepared.charts,
            ["chart-main.png", "https://shared.example/chart.png"]
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_flat_pattern_counts_by_its_headings_past_the_title() {
        let path = write_temp("needles-flat.md", b"# Dishcloth\n\n# Body\n\n# Edging\n");
        let prepared = prepare(&path, None).expect("prepared");
        assert_eq!(prepared.sections, ["Body", "Edging"]);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn a_title_alone_is_not_a_section_list() {
        let path = write_temp("needles-bare.md", b"# Just a title\n\nPlain rows.\n");
        let prepared = prepare(&path, None).expect("prepared");
        assert!(prepared.sections.is_empty());
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn title_flags_come_off_the_argument_list() {
        let (rest, title) = take_title(&[
            "in.md".to_owned(),
            "--title".to_owned(),
            "Winter socks".to_owned(),
            "--sim".to_owned(),
        ]);
        assert_eq!(rest, ["in.md", "--sim"]);
        assert_eq!(title.as_deref(), Some("Winter socks"));
    }

    #[test]
    fn a_push_to_the_simulator_lays_the_pattern_on_its_shelf() {
        let path = write_temp("needles-sim.md", b"# Simmed\n\n## Cuff\n\nRows.\n");
        let prepared = prepare(&path, None).expect("prepared");
        super::transfer_sim(&prepared, &path).expect("transfer");
        let written =
            std::fs::read(kobo_sim::simulated_data_root("needles").join(BLOB)).expect("shelf blob");
        assert_eq!(written, prepared.markdown);
        std::fs::remove_file(path).expect("cleanup");
        std::fs::remove_file(kobo_sim::simulated_data_root("needles").join(BLOB)).expect("cleanup");
    }
}
