//! Owner-attended preparation and transfer for Needles pattern documents.
//!
//! PDF parsing belongs on the host: this keeps the reader application small,
//! lets the shared book reader handle reflow, and never sends credentials here.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_PDF: usize = 32 * 1024 * 1024;
const MAX_PATTERN: usize = 4 * 1024 * 1024;
const BLOB: &str = "pattern.md";
const USAGE: &str = "usage: kobo needles prepare PATTERN.pdf --out PATTERN.md\n\
                     \x20      kobo needles push PATTERN.(pdf|md|txt) --device IP";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments {
        [verb, input, flag, output] if verb == "prepare" && flag == "--out" => {
            let markdown = prepare(Path::new(input))?;
            std::fs::write(output, markdown)
                .map_err(|error| format!("could not write {output}: {error}"))?;
            println!("Prepared Needles pattern: {output}");
            Ok(())
        }
        [verb, input, device, host] if verb == "push" && super::is_device_flag(device) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            let path = Path::new(input);
            let markdown = if has_extension(path, "pdf") {
                prepare(path)?
            } else if has_text_extension(path) {
                read_pattern(path)?
            } else {
                return Err("Needles accepts a .pdf, .md or .txt pattern file".to_owned());
            };
            transfer(&markdown, host)
        }
        _ => Err(USAGE.to_owned()),
    }
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
         printf 'Transferred Needles pattern\\n'\n"
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

fn has_text_extension(path: &Path) -> bool {
    has_extension(path, "md") || has_extension(path, "txt")
}

#[cfg(test)]
mod tests {
    use super::{has_extension, has_text_extension, read_limited, read_pattern, BLOB};
    use std::io::Cursor;
    use std::path::Path;

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
