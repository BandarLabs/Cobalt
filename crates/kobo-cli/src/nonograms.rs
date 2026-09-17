//! Safe host-side preparation and transfer for Nonograms photo puzzles.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/nonograms";
const BLOB: &str = "photo.png";
const NORMALIZED_EDGE: u32 = 360;
const MAX_TRANSFER_BYTES: usize = 256 * 1024;
const PLAYABLE_SIDES: [usize; 3] = [5, 7, 9];
const USAGE: &str = "usage:
  kobo nonograms preview IMAGE --out DIRECTORY
  kobo nonograms push IMAGE --size N (--device IP | --out photo.png)
N is exactly 5, 7, or 9. Preview checks all three sizes without transferring.";

#[derive(Debug, Eq, PartialEq)]
enum Destination<'a> {
    Device(&'a str),
    Output(&'a str),
}

#[derive(Debug, Eq, PartialEq)]
struct Push<'a> {
    input: &'a str,
    side: usize,
    destination: Destination<'a>,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    if arguments.first().map(String::as_str) == Some("preview") {
        return preview_command(arguments);
    }
    let push = parse_push(arguments)?;
    let source = read_image(Path::new(push.input))?;
    let png = prepare(&source, push.side)?;
    let analysis = analyse(&png, push.side)?;
    match push.destination {
        Destination::Device(host) => transfer(&png, host),
        Destination::Output(path) => {
            write_output(Path::new(path), &png)?;
            println!(
                "Prepared Nonograms photo: {path} ({} solving passes)",
                analysis.rounds
            );
            Ok(())
        }
    }
}

fn parse_push(arguments: &[String]) -> Result<Push<'_>, String> {
    if arguments.first().map(String::as_str) != Some("push") {
        return Err(USAGE.to_owned());
    }
    let input = arguments.get(1).ok_or_else(|| USAGE.to_owned())?;
    let mut side = None;
    let mut destination = None;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--size" => {
                if side.is_some() {
                    return Err(USAGE.to_owned());
                }
                side = Some(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| USAGE.to_owned())?
                        .parse()
                        .map_err(|_| size_error())?,
                );
                index += 2;
            }
            flag if super::is_device_flag(flag) => {
                let host = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                if !super::valid_device_host(host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                if destination.replace(Destination::Device(host)).is_some() {
                    return Err(USAGE.to_owned());
                }
                index += 2;
            }
            "--out" => {
                let output = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                if destination.replace(Destination::Output(output)).is_some() {
                    return Err(USAGE.to_owned());
                }
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let side = side.ok_or_else(|| USAGE.to_owned())?;
    if !PLAYABLE_SIDES.contains(&side) {
        return Err(size_error());
    }
    let destination = destination.ok_or_else(|| USAGE.to_owned())?;
    if let Destination::Output(path) = &destination {
        validate_output_path(Path::new(path))?;
    }
    Ok(Push {
        input,
        side,
        destination,
    })
}

fn read_image(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular image file", path.display()));
    }
    if metadata.len() > kobo_image::MAX_SOURCE_BYTES as u64 {
        return Err(format!(
            "{} is larger than the {} MB image limit",
            path.display(),
            kobo_image::MAX_SOURCE_BYTES / (1024 * 1024)
        ));
    }
    let mut source =
        File::open(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    Read::take(&mut source, kobo_image::MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() > kobo_image::MAX_SOURCE_BYTES {
        return Err(format!(
            "{} is larger than the {} MB image limit",
            path.display(),
            kobo_image::MAX_SOURCE_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
}

fn prepare(source: &[u8], side: usize) -> Result<Vec<u8>, String> {
    if !PLAYABLE_SIDES.contains(&side) {
        return Err(size_error());
    }
    let picture = kobo_image::decode(source).map_err(|error| format!("decode image: {error}"))?;
    let picture = picture
        .cover(NORMALIZED_EDGE, NORMALIZED_EDGE)
        .map_err(|error| format!("crop image: {error}"))?;
    let grey = picture
        .grey()
        .iter()
        .map(|pixel| if *pixel < 128 { 0 } else { u8::MAX })
        .collect::<Vec<_>>();
    let png = kobo_image::encode_png_grey(NORMALIZED_EDGE, NORMALIZED_EDGE, &grey)
        .map_err(|error| format!("encode image: {error}"))?;
    let png = add_source_identity(png, source)?;
    if png.len() > MAX_TRANSFER_BYTES {
        return Err("the prepared photo is too large for the reader".to_owned());
    }
    Ok(png)
}

#[derive(Clone, Debug)]
struct Analysis {
    side: usize,
    answer: Vec<bool>,
    row_clues: Vec<Vec<usize>>,
    column_clues: Vec<Vec<usize>>,
    rounds: usize,
}

fn preview_command(arguments: &[String]) -> Result<(), String> {
    let [_, input, flag, output] = arguments else {
        return Err(USAGE.to_owned());
    };
    if flag != "--out" {
        return Err(USAGE.to_owned());
    }
    let output = Path::new(output);
    if output.exists() {
        return Err(format!(
            "preview directory {} already exists",
            output.display()
        ));
    }
    let source = read_image(Path::new(input))?;
    let mut prepared = Vec::new();
    for side in PLAYABLE_SIDES {
        let png = prepare(&source, side)?;
        let analysis = analyse(&png, side)?;
        prepared.push((png, analysis));
    }
    fs::create_dir(output)
        .map_err(|error| format!("create preview directory {}: {error}", output.display()))?;
    if let Err(error) = write_preview(output, &prepared) {
        let _ = fs::remove_dir_all(output);
        return Err(error);
    }
    println!(
        "Previewed a fair Nonograms puzzle at 5 × 5, 7 × 7 and 9 × 9. Open {}.",
        output.join("index.html").display()
    );
    Ok(())
}

fn analyse(png: &[u8], side: usize) -> Result<Analysis, String> {
    let picture =
        kobo_image::decode(png).map_err(|error| format!("decode prepared image: {error}"))?;
    let width = picture.width() as usize;
    let height = picture.height() as usize;
    let pixels = picture.grey();
    let samples = (0..side)
        .flat_map(|y| {
            (0..side).map(move |x| {
                let left = x * width / side;
                let right = ((x + 1) * width / side).max(left + 1);
                let top = y * height / side;
                let bottom = ((y + 1) * height / side).max(top + 1);
                let mut total = 0_u64;
                let mut count = 0_u64;
                for row in top..bottom {
                    for column in left..right {
                        total += u64::from(pixels[row * width + column]);
                        count += 1;
                    }
                }
                u8::try_from(total / count.max(1)).unwrap_or(u8::MAX)
            })
        })
        .collect::<Vec<_>>();
    let mean = samples.iter().map(|value| u32::from(*value)).sum::<u32>()
        / u32::try_from(samples.len()).unwrap_or(1);
    for offset in [-48_i32, -32, -16, 0, 16, 32, 48] {
        let threshold = u8::try_from((i32::try_from(mean).unwrap_or(128) + offset).clamp(16, 239))
            .unwrap_or(128);
        let answer = samples
            .iter()
            .map(|value| *value < threshold)
            .collect::<Vec<_>>();
        let row_clues = (0..side)
            .map(|row| runs(&answer[row * side..(row + 1) * side]))
            .collect::<Vec<_>>();
        let column_clues = (0..side)
            .map(|column| {
                runs(
                    (0..side)
                        .map(|row| answer[row * side + column])
                        .collect::<Vec<_>>()
                        .as_slice(),
                )
            })
            .collect::<Vec<_>>();
        if let Some(rounds) = solve(side, &row_clues, &column_clues) {
            return Ok(Analysis {
                side,
                answer,
                row_clues,
                column_clues,
                rounds,
            });
        }
    }
    Err(format!(
        "This photo does not make a fair {side} × {side} puzzle without guessing"
    ))
}

fn runs(line: &[bool]) -> Vec<usize> {
    let mut output = Vec::new();
    let mut count = 0;
    for filled in line.iter().copied().chain(std::iter::once(false)) {
        if filled {
            count += 1;
        } else if count != 0 {
            output.push(count);
            count = 0;
        }
    }
    output
}

fn line_candidates(length: usize, clues: &[usize], known: &[Option<bool>]) -> Vec<Vec<bool>> {
    fn place(
        at: usize,
        clue: usize,
        clues: &[usize],
        line: &mut [bool],
        known: &[Option<bool>],
        out: &mut Vec<Vec<bool>>,
    ) {
        if clue == clues.len() {
            for cell in &mut line[at..] {
                *cell = false;
            }
            if line
                .iter()
                .zip(known)
                .all(|(cell, known)| known.is_none_or(|value| value == *cell))
            {
                out.push(line.to_vec());
            }
            return;
        }
        let run = clues[clue];
        let later = clues[clue + 1..].iter().sum::<usize>() + clues.len().saturating_sub(clue + 2);
        for start in at..=line.len().saturating_sub(run + later) {
            let saved = line.to_vec();
            for cell in &mut line[at..start] {
                *cell = false;
            }
            for cell in &mut line[start..start + run] {
                *cell = true;
            }
            let next = start + run;
            if clue + 1 < clues.len() {
                line[next] = false;
                place(next + 1, clue + 1, clues, line, known, out);
            } else {
                place(next, clue + 1, clues, line, known, out);
            }
            line.copy_from_slice(&saved);
        }
    }
    let mut output = Vec::new();
    place(0, 0, clues, &mut vec![false; length], known, &mut output);
    output
}

fn solve(side: usize, rows: &[Vec<usize>], columns: &[Vec<usize>]) -> Option<usize> {
    let mut board = vec![None; side * side];
    let mut rounds = 0;
    loop {
        let mut changed = false;
        for vertical in [false, true] {
            let groups = if vertical { columns } else { rows };
            for (line, clues) in groups.iter().enumerate() {
                let known = (0..side)
                    .map(|index| {
                        board[if vertical {
                            index * side + line
                        } else {
                            line * side + index
                        }]
                    })
                    .collect::<Vec<_>>();
                let candidates = line_candidates(side, clues, &known);
                let first = candidates.first()?;
                for index in 0..side {
                    if candidates
                        .iter()
                        .all(|candidate| candidate[index] == first[index])
                    {
                        let at = if vertical {
                            index * side + line
                        } else {
                            line * side + index
                        };
                        if board[at].is_none() {
                            board[at] = Some(first[index]);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            return board.iter().all(Option::is_some).then_some(rounds);
        }
        rounds += 1;
    }
}

fn write_preview(output: &Path, puzzles: &[(Vec<u8>, Analysis)]) -> Result<(), String> {
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Nonograms preview</title><style>body{font:17px/1.45 system-ui,sans-serif;max-width:940px;margin:auto;padding:24px;color:#222;background:#f5f3ed}section{margin:32px 0}.grid{display:grid;width:min(70vw,420px);aspect-ratio:1;border:2px solid #222}.cell{border:1px solid #aaa;background:white}.fill{background:#222}.clues{overflow-wrap:anywhere}</style><main><h1>Nonograms preview</h1><p>These puzzles passed the same bounded line-solving rule used by the reader. Nothing was transferred.</p>",
    );
    for (png, puzzle) in puzzles {
        let filename = format!("photo-{}.png", puzzle.side);
        fs::write(output.join(&filename), png)
            .map_err(|error| format!("write preview image: {error}"))?;
        write!(html, "<section><h2>{0} × {0}</h2><p>{1} solving passes</p><div class=\"grid\" style=\"grid-template-columns:repeat({0},1fr)\">", puzzle.side, puzzle.rounds).unwrap();
        for filled in &puzzle.answer {
            html.push_str(if *filled {
                "<span class=\"cell fill\"></span>"
            } else {
                "<span class=\"cell\"></span>"
            });
        }
        html.push_str("</div><p class=\"clues\">Rows: ");
        for (index, clues) in puzzle.row_clues.iter().enumerate() {
            if index != 0 {
                html.push_str(" · ");
            }
            write!(
                html,
                "{}",
                clues
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap();
        }
        html.push_str("</p><p class=\"clues\">Columns: ");
        for (index, clues) in puzzle.column_clues.iter().enumerate() {
            if index != 0 {
                html.push_str(" · ");
            }
            write!(
                html,
                "{}",
                clues
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap();
        }
        write!(
            html,
            "</p><p><a href=\"{filename}\">Prepared reader image</a></p></section>"
        )
        .unwrap();
    }
    fs::write(output.join("index.html"), format!("{html}</main></html>"))
        .map_err(|error| format!("write preview page: {error}"))
}

fn size_error() -> String {
    "Nonograms --size must be exactly 5, 7, or 9".to_owned()
}

fn add_source_identity(mut png: Vec<u8>, source: &[u8]) -> Result<Vec<u8>, String> {
    const IEND_BYTES: usize = 12;
    const KEYWORD: &[u8] = b"Cobalt-Nonograms-Source";
    if png.len() < IEND_BYTES || &png[png.len() - 8..png.len() - 4] != b"IEND" {
        return Err("the image encoder did not produce a complete PNG".to_owned());
    }
    let value = kobo_net::sha256::hex_digest(source);
    let mut text = Vec::with_capacity(KEYWORD.len() + 1 + value.len());
    text.extend_from_slice(KEYWORD);
    text.push(0);
    text.extend_from_slice(value.as_bytes());

    let insert = png.len() - IEND_BYTES;
    let mut chunk = Vec::with_capacity(12 + text.len());
    chunk.extend_from_slice(&u32::try_from(text.len()).unwrap_or(u32::MAX).to_be_bytes());
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&text);
    let crc = png_crc32(&chunk[4..]);
    chunk.extend_from_slice(&crc.to_be_bytes());
    png.splice(insert..insert, chunk);
    Ok(png)
}

fn png_crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb8_8320
            };
        }
    }
    !crc
}

fn validate_output_path(path: &Path) -> Result<(), String> {
    if path.file_name().and_then(|name| name.to_str()) != Some(BLOB) {
        return Err("--out must name photo.png, the file Nonograms reads".to_owned());
    }
    let parent = output_parent(path);
    if !parent.is_dir() {
        return Err(format!(
            "output directory {} does not exist",
            parent.display()
        ));
    }
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(format!("refusing to replace symlink {}", path.display()));
    }
    Ok(())
}

fn write_output(path: &Path, png: &[u8]) -> Result<(), String> {
    validate_output_path(path)?;
    let parent = output_parent(path);
    let temporary = parent.join(format!(".{BLOB}.{}.writing", std::process::id()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    if let Err(error) = output.write_all(png).and_then(|()| output.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write {}: {error}", temporary.display()));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("replace {}: {error}", path.display()));
    }
    Ok(())
}

fn output_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn transfer(png: &[u8], host: &str) -> Result<(), String> {
    let script = transfer_script(png);
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the Nonograms photo transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn transfer_script(png: &[u8]) -> String {
    let encoded = super::base64_encode(png);
    let bytes = png.len();
    let sha256 = kobo_net::sha256::hex_digest(png);
    format!(
        "set -eu\n\
         root='{ROOT}'\n\
         mkdir -p \"$root\"\n\
         chmod 700 \"$root\"\n\
         partial=\"$root/.{BLOB}.$$.writing\"\n\
         final=\"$root/{BLOB}\"\n\
         trap 'rm -f \"$partial\"' EXIT HUP INT TERM\n\
         base64 -d > \"$partial\" <<'KOBO_NONOGRAMS_PHOTO'\n\
         {encoded}\n\
         KOBO_NONOGRAMS_PHOTO\n\
         chmod 600 \"$partial\"\n\
         test \"$(wc -c < \"$partial\")\" = '{bytes}'\n\
         set -- $(sha256sum \"$partial\")\n\
         test \"$1\" = '{sha256}'\n\
         mv -f \"$partial\" \"$final\"\n\
         test \"$(wc -c < \"$final\")\" = '{bytes}'\n\
         set -- $(sha256sum \"$final\")\n\
         test \"$1\" = '{sha256}'\n\
         sync\n\
         printf 'Transferred Nonograms photo\\n'\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        analyse, parse_push, prepare, preview_command, size_error, transfer_script, Destination,
        Push, BLOB, NORMALIZED_EDGE,
    };
    use kobo_image::Picture;
    use std::fs;

    #[test]
    fn parses_a_bounded_push_destination_and_size() {
        let arguments = vec![
            "push".into(),
            "photo.jpg".into(),
            "--size".into(),
            "9".into(),
            "--out".into(),
            "photo.png".into(),
        ];
        assert_eq!(
            parse_push(&arguments).expect("parse"),
            Push {
                input: "photo.jpg",
                side: 9,
                destination: Destination::Output("photo.png"),
            }
        );
        for side in ["6", "8", "10"] {
            let invalid = vec![
                "push".into(),
                "photo.jpg".into(),
                "--size".into(),
                side.into(),
                "--out".into(),
                "photo.png".into(),
            ];
            assert_eq!(parse_push(&invalid).unwrap_err(), size_error());
        }
        let duplicate_size = vec![
            "push".into(),
            "photo.jpg".into(),
            "--size".into(),
            "5".into(),
            "--size".into(),
            "9".into(),
            "--out".into(),
            "photo.png".into(),
        ];
        assert!(parse_push(&duplicate_size).is_err());
        let unsafe_output = vec![
            "push".into(),
            "photo.jpg".into(),
            "--size".into(),
            "9".into(),
            "--out".into(),
            "other.png".into(),
        ];
        assert!(parse_push(&unsafe_output).is_err());
    }

    #[test]
    fn main_dispatches_to_nonograms_command() {
        super::super::run(&["nonograms".to_owned(), "--help".to_owned()]).expect("help");
        let error =
            super::super::run(&["nonograms".to_owned(), "look".to_owned()]).expect_err("usage");
        assert!(error.starts_with("usage:\n  kobo nonograms preview"));
    }

    #[test]
    fn normalizes_common_image_input_to_a_deterministic_bounded_png() {
        let source = grey_png(2, 2, &[0, 64, 192, 255]);
        let first = prepare(&source, 9).expect("prepare");
        let second = prepare(&source, 9).expect("prepare again");
        assert_eq!(first, second);
        assert_ne!(
            first,
            prepare(&grey_png(2, 2, &[255; 4]), 9).expect("other photo")
        );
        let prepared = kobo_image::decode(&first).expect("prepared png");
        assert_eq!(
            (prepared.width(), prepared.height()),
            (NORMALIZED_EDGE, NORMALIZED_EDGE)
        );
        assert!(prepared
            .grey()
            .iter()
            .all(|pixel| matches!(*pixel, 0 | u8::MAX)));
        assert!(prepare(b"not an image", 9).is_err());
    }

    #[test]
    fn concurrent_transfers_use_pid_scoped_atomic_photo_scratch_files() {
        let script = transfer_script(b"photo");
        assert!(script.contains("/data/nonograms"));
        assert!(script.contains("partial=\"$root/.photo.png.$$.writing\""));
        assert!(script.contains("trap 'rm -f \"$partial\"' EXIT HUP INT TERM"));
        assert!(script.contains("mv -f \"$partial\" \"$final\""));
        assert_eq!(script.matches("sha256sum").count(), 2);
        assert_eq!(script.matches("wc -c").count(), 2);
        assert!(!script.contains(".photo.png.writing"));
        assert!(script.contains("KOBO_NONOGRAMS_PHOTO"));
        assert_eq!(BLOB, "photo.png");
    }

    #[test]
    fn analyses_fair_sizes_and_refuses_guessing_patterns() {
        let grey = (0..100)
            .map(|index| if index / 10 < 5 { 24 } else { 232 })
            .collect::<Vec<_>>();
        let source = grey_png(10, 10, &grey);
        for side in [5, 7, 9] {
            let prepared = prepare(&source, side).expect("prepare");
            let analysis = analyse(&prepared, side).expect("fair analysis");
            assert_eq!(analysis.side, side);
            assert_eq!(analysis.answer.len(), side * side);
        }
        let diagonal = grey_png(
            5,
            5,
            &(0..25)
                .map(|index| if index / 5 == index % 5 { 0 } else { 255 })
                .collect::<Vec<_>>(),
        );
        let prepared = prepare(&diagonal, 5).expect("prepare diagonal");
        assert!(analyse(&prepared, 5).is_err());
    }

    #[test]
    fn preview_is_atomic_and_contains_all_supported_sizes() {
        let root = std::env::temp_dir().join(format!("nonograms-preview-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let input = root.join("source.png");
        let grey = (0..100)
            .map(|index| if index / 10 < 5 { 24 } else { 232 })
            .collect::<Vec<_>>();
        fs::write(&input, grey_png(10, 10, &grey)).unwrap();
        let output = root.join("preview");
        preview_command(&[
            "preview".into(),
            input.display().to_string(),
            "--out".into(),
            output.display().to_string(),
        ])
        .unwrap();
        let html = fs::read_to_string(output.join("index.html")).unwrap();
        for side in [5, 7, 9] {
            assert!(html.contains(&format!("{side} × {side}")));
            assert!(output.join(format!("photo-{side}.png")).is_file());
        }
        assert!(preview_command(&[
            "preview".into(),
            input.display().to_string(),
            "--out".into(),
            output.display().to_string(),
        ])
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
    }

    fn grey_png(width: u32, height: u32, grey: &[u8]) -> Vec<u8> {
        let picture = Picture::from_grey(width, height, grey.to_vec()).expect("picture");
        kobo_image::encode_png_grey(picture.width(), picture.height(), picture.grey()).expect("png")
    }
}
