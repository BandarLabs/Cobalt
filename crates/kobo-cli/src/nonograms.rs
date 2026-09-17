//! Safe host-side preparation and transfer for Nonograms photo puzzles.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/nonograms";
const MANIFEST: &str = "imported.txt";
const NORMALIZED_EDGE: u32 = 360;
const MAX_TRANSFER_BYTES: usize = 256 * 1024;
const MIN_SIDE: usize = 5;
const MAX_SIDE: usize = 25;
const MAX_IMPORTED: usize = 12;
const MAX_NAME: usize = 48;
const USAGE: &str = "usage:
  kobo nonograms preview IMAGE --out DIRECTORY
  kobo nonograms push IMAGE --name NAME --size N [--add IMAGE --name NAME --size N]... (--sim | --device IP | --out DIRECTORY)
N is from 5 to 25. A push synchronizes the named imported-puzzle shelf.";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Destination<'a> {
    Device(&'a str),
    Simulator,
    Output(&'a str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Import<'a> {
    input: &'a str,
    name: &'a str,
    side: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Push<'a> {
    imports: Vec<Import<'a>>,
    destination: Destination<'a>,
}

#[derive(Clone, Debug)]
struct Prepared {
    file: String,
    name: String,
    side: usize,
    png: Vec<u8>,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    if arguments.first().map(String::as_str) == Some("preview") {
        return preview_command(arguments);
    }
    let push = parse_push(arguments)?;
    let prepared = prepare_imports(&push.imports)?;
    publish(&prepared, &push.destination)?;
    println!(
        "Synchronized {} Nonograms {}.",
        prepared.len(),
        if prepared.len() == 1 {
            "puzzle"
        } else {
            "puzzles"
        }
    );
    Ok(())
}

fn parse_push(arguments: &[String]) -> Result<Push<'_>, String> {
    if arguments.first().map(String::as_str) != Some("push") {
        return Err(USAGE.to_owned());
    }
    let first = arguments.get(1).ok_or_else(|| USAGE.to_owned())?;
    let mut imports = vec![Import {
        input: first,
        name: "",
        side: 0,
    }];
    let mut destination = None;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--add" => {
                let input = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                if imports.len() == MAX_IMPORTED {
                    return Err(format!(
                        "a Nonograms push accepts at most {MAX_IMPORTED} puzzles"
                    ));
                }
                imports.push(Import {
                    input,
                    name: "",
                    side: 0,
                });
                index += 2;
            }
            "--name" => {
                let name = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                if !imports.last().is_some_and(|entry| entry.name.is_empty()) {
                    return Err(USAGE.to_owned());
                }
                imports.last_mut().expect("first import exists").name = name;
                index += 2;
            }
            "--size" => {
                let side = arguments
                    .get(index + 1)
                    .ok_or_else(|| USAGE.to_owned())?
                    .parse()
                    .map_err(|_| size_error())?;
                if imports.last().is_none_or(|entry| entry.side != 0) {
                    return Err(USAGE.to_owned());
                }
                imports.last_mut().expect("first import exists").side = side;
                index += 2;
            }
            flag if super::is_device_flag(flag) => {
                let host = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                if !super::valid_device_host(host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                set_destination(&mut destination, Destination::Device(host))?;
                index += 2;
            }
            "--sim" => {
                set_destination(&mut destination, Destination::Simulator)?;
                index += 1;
            }
            "--out" => {
                let output = arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?;
                set_destination(&mut destination, Destination::Output(output))?;
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    for import in &imports {
        validate_name(import.name)?;
        validate_side(import.side)?;
    }
    Ok(Push {
        imports,
        destination: destination.ok_or_else(|| USAGE.to_owned())?,
    })
}

fn set_destination<'a>(
    slot: &mut Option<Destination<'a>>,
    value: Destination<'a>,
) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(USAGE.to_owned());
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    if name.trim().is_empty() || count > MAX_NAME || name.contains(['\n', '\r', '\t']) {
        return Err(format!(
            "puzzle names must be 1 to {MAX_NAME} characters without tabs or newlines"
        ));
    }
    Ok(())
}

fn validate_side(side: usize) -> Result<(), String> {
    if !(MIN_SIDE..=MAX_SIDE).contains(&side) {
        return Err(size_error());
    }
    Ok(())
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
        return Err(format!("{} is larger than the image limit", path.display()));
    }
    Ok(bytes)
}

fn prepare_imports(imports: &[Import<'_>]) -> Result<Vec<Prepared>, String> {
    let mut output = Vec::with_capacity(imports.len());
    for import in imports {
        let source = read_image(Path::new(import.input))?;
        let png = prepare(&source)?;
        analyse(&png, import.side)?;
        let digest = kobo_net::sha256::hex_digest(&source);
        let file = format!("photo-{}-{}.png", import.side, &digest[..24]);
        output.push(Prepared {
            file,
            name: import.name.trim().to_owned(),
            side: import.side,
            png,
        });
    }
    Ok(output)
}

fn prepare(source: &[u8]) -> Result<Vec<u8>, String> {
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

fn manifest(prepared: &[Prepared]) -> String {
    let mut output = String::new();
    for entry in prepared {
        writeln!(output, "{}\t{}\t{}", entry.file, entry.name, entry.side).unwrap();
    }
    output
}

fn publish(prepared: &[Prepared], destination: &Destination<'_>) -> Result<(), String> {
    match destination {
        Destination::Simulator => publish_local(
            &std::env::temp_dir().join("cobalt-sim-data/nonograms"),
            prepared,
        ),
        Destination::Output(path) => publish_local(Path::new(path), prepared),
        Destination::Device(host) => transfer(prepared, host),
    }
}

fn publish_local(root: &Path, prepared: &[Prepared]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("create {}: {error}", root.display()))?;
    let staging = root.join(format!(".nonograms-import-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| format!("clear staging: {error}"))?;
    }
    fs::create_dir(&staging).map_err(|error| format!("create staging: {error}"))?;
    let result = (|| {
        for entry in prepared {
            fs::write(staging.join(&entry.file), &entry.png)
                .map_err(|error| format!("write {}: {error}", entry.file))?;
        }
        fs::write(staging.join(MANIFEST), manifest(prepared))
            .map_err(|error| format!("write {MANIFEST}: {error}"))?;
        for entry in prepared {
            fs::rename(staging.join(&entry.file), root.join(&entry.file))
                .map_err(|error| format!("publish {}: {error}", entry.file))?;
        }
        fs::rename(staging.join(MANIFEST), root.join(MANIFEST))
            .map_err(|error| format!("publish {MANIFEST}: {error}"))?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging);
    result
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
    let png = prepare(&source)?;
    let mut accepted = Vec::new();
    let mut refused = Vec::new();
    for side in MIN_SIDE..=MAX_SIDE {
        match analyse(&png, side) {
            Ok(analysis) => accepted.push(analysis),
            Err(_) => refused.push(side),
        }
    }
    if accepted.is_empty() {
        return Err("This photo does not make a fair puzzle at any supported size".to_owned());
    }
    fs::create_dir(output)
        .map_err(|error| format!("create preview directory {}: {error}", output.display()))?;
    if let Err(error) = write_preview(output, &png, &accepted, &refused) {
        let _ = fs::remove_dir_all(output);
        return Err(error);
    }
    println!(
        "Previewed {} fair sizes from 5 × 5 through 25 × 25. Open {}.",
        accepted.len(),
        output.join("index.html").display()
    );
    Ok(())
}

fn analyse(png: &[u8], side: usize) -> Result<Analysis, String> {
    validate_side(side)?;
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
    let mean = samples.iter().map(|v| u32::from(*v)).sum::<u32>()
        / u32::try_from(samples.len()).unwrap_or(1);
    for offset in [-48_i32, -32, -16, 0, 16, 32, 48] {
        let threshold = u8::try_from((i32::try_from(mean).unwrap_or(128) + offset).clamp(16, 239))
            .unwrap_or(128);
        let answer = samples.iter().map(|v| *v < threshold).collect::<Vec<_>>();
        let row_clues = (0..side)
            .map(|r| runs(&answer[r * side..(r + 1) * side]))
            .collect::<Vec<_>>();
        let column_clues = (0..side)
            .map(|c| runs(&(0..side).map(|r| answer[r * side + c]).collect::<Vec<_>>()))
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
    let mut out = Vec::new();
    let mut n = 0;
    for filled in line.iter().copied().chain(std::iter::once(false)) {
        if filled {
            n += 1;
        } else if n != 0 {
            out.push(n);
            n = 0;
        }
    }
    out
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
                .all(|(cell, known)| known.is_none_or(|v| v == *cell))
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
    let mut out = Vec::new();
    place(0, 0, clues, &mut vec![false; length], known, &mut out);
    out
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
                    .map(|i| {
                        board[if vertical {
                            i * side + line
                        } else {
                            line * side + i
                        }]
                    })
                    .collect::<Vec<_>>();
                let candidates = line_candidates(side, clues, &known);
                let first = candidates.first()?;
                for i in 0..side {
                    if candidates.iter().all(|c| c[i] == first[i]) {
                        let at = if vertical {
                            i * side + line
                        } else {
                            line * side + i
                        };
                        if board[at].is_none() {
                            board[at] = Some(first[i]);
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

fn write_preview(
    output: &Path,
    png: &[u8],
    puzzles: &[Analysis],
    refused: &[usize],
) -> Result<(), String> {
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Nonograms preview</title><style>body{font:17px/1.45 system-ui,sans-serif;max-width:940px;margin:auto;padding:24px;color:#222;background:#f5f3ed}section{margin:32px 0}.grid{display:grid;width:min(70vw,420px);aspect-ratio:1;border:2px solid #222}.cell{border:1px solid #aaa;background:white}.fill{background:#222}.clues{overflow-wrap:anywhere}</style><main><h1>Nonograms preview</h1><p>Accepted sizes passed the same bounded line-solving rule used by the reader. Nothing was transferred.</p>",
    );
    if !refused.is_empty() {
        write!(
            html,
            "<p>Needs guessing and will be refused: {}</p>",
            refused
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .unwrap();
    }
    for p in puzzles {
        write!(html,"<section><h2>{0} × {0}</h2><p>{1} solving passes</p><div class=\"grid\" style=\"grid-template-columns:repeat({0},1fr)\">",p.side,p.rounds).unwrap();
        for filled in &p.answer {
            html.push_str(if *filled {
                "<span class=\"cell fill\"></span>"
            } else {
                "<span class=\"cell\"></span>"
            });
        }
        html.push_str("</div><p class=\"clues\">Rows: ");
        for (i, c) in p.row_clues.iter().enumerate() {
            if i != 0 {
                html.push_str(" · ");
            }
            write!(
                html,
                "{}",
                c.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
            )
            .unwrap();
        }
        html.push_str("</p><p class=\"clues\">Columns: ");
        for (i, c) in p.column_clues.iter().enumerate() {
            if i != 0 {
                html.push_str(" · ");
            }
            write!(
                html,
                "{}",
                c.iter().map(usize::to_string).collect::<Vec<_>>().join(",")
            )
            .unwrap();
        }
        html.push_str("</p></section>");
    }
    fs::write(output.join("prepared.png"), png).map_err(|e| format!("write preview image: {e}"))?;
    fs::write(output.join("index.html"), format!("{html}</main></html>"))
        .map_err(|e| format!("write preview page: {e}"))
}

fn size_error() -> String {
    format!("Nonograms --size must be from {MIN_SIDE} to {MAX_SIDE}")
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
            }
        }
    }
    !crc
}

fn transfer(prepared: &[Prepared], host: &str) -> Result<(), String> {
    let script = transfer_script(prepared);
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the Nonograms transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}
fn transfer_script(prepared: &[Prepared]) -> String {
    let mut script = format!(
        "set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\nstage=\"$root/.import.$$\"\nmkdir \"$stage\"\ntrap 'rm -rf \"$stage\"' EXIT HUP INT TERM\n"
    );
    for entry in prepared {
        let encoded = super::base64_encode(&entry.png);
        let bytes = entry.png.len();
        let sha = kobo_net::sha256::hex_digest(&entry.png);
        writeln!(script,"base64 -d > \"$stage/{}\" <<'KOBO_NONOGRAM_PHOTO'\n{}\nKOBO_NONOGRAM_PHOTO\ntest \"$(wc -c < \"$stage/{}\")\" = '{}'\nset -- $(sha256sum \"$stage/{}\"); test \"$1\" = '{}'\n",entry.file,encoded,entry.file,bytes,entry.file,sha).unwrap();
    }
    let encoded = super::base64_encode(manifest(prepared).as_bytes());
    writeln!(script,"base64 -d > \"$stage/{MANIFEST}\" <<'KOBO_NONOGRAM_MANIFEST'\n{encoded}\nKOBO_NONOGRAM_MANIFEST").unwrap();
    for entry in prepared {
        writeln!(
            script,
            "mv -f \"$stage/{}\" \"$root/{}\"",
            entry.file, entry.file
        )
        .unwrap();
    }
    writeln!(script,"mv -f \"$stage/{MANIFEST}\" \"$root/{MANIFEST}\"\nsync\nprintf 'Transferred {} Nonograms puzzles\\n'",prepared.len()).unwrap();
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_image::Picture;

    #[test]
    fn parses_named_multi_puzzle_sync_and_all_supported_sizes() {
        let args = vec![
            "push".into(),
            "a.png".into(),
            "--name".into(),
            "Moon".into(),
            "--size".into(),
            "5".into(),
            "--add".into(),
            "b.png".into(),
            "--name".into(),
            "Earth".into(),
            "--size".into(),
            "25".into(),
            "--sim".into(),
        ];
        let push = parse_push(&args).unwrap();
        assert_eq!(push.imports.len(), 2);
        assert_eq!(push.imports[1].side, 25);
        assert_eq!(push.destination, Destination::Simulator);
        for side in [4, 26] {
            let mut bad = args.clone();
            bad[5] = side.to_string();
            assert!(parse_push(&bad).is_err());
        }
    }

    #[test]
    fn names_and_count_are_bounded() {
        let bad = vec![
            "push".into(),
            "a.png".into(),
            "--name".into(),
            "x".repeat(49),
            "--size".into(),
            "5".into(),
            "--sim".into(),
        ];
        assert!(parse_push(&bad).is_err());
        let mut too_many = vec![
            "push".into(),
            "a.png".into(),
            "--name".into(),
            "0".into(),
            "--size".into(),
            "5".into(),
        ];
        for n in 1..=12 {
            too_many.extend([
                "--add".into(),
                "a.png".into(),
                "--name".into(),
                n.to_string(),
                "--size".into(),
                "5".into(),
            ]);
        }
        too_many.push("--sim".into());
        assert!(parse_push(&too_many).is_err());
    }

    #[test]
    fn analyses_fair_sizes_and_refuses_guessing_patterns() {
        let source = grey_png(
            10,
            10,
            &(0..100)
                .map(|i| if i / 10 < 5 { 24 } else { 232 })
                .collect::<Vec<_>>(),
        );
        let prepared = prepare(&source).unwrap();
        for side in [5, 7, 9, 15, 25] {
            assert_eq!(analyse(&prepared, side).unwrap().side, side);
        }
        let diagonal = grey_png(
            5,
            5,
            &(0..25)
                .map(|i| if i / 5 == i % 5 { 0 } else { 255 })
                .collect::<Vec<_>>(),
        );
        assert!(analyse(&prepare(&diagonal).unwrap(), 5).is_err());
    }

    #[test]
    fn local_publish_writes_images_before_the_sync_manifest() {
        let root = std::env::temp_dir().join(format!("nonograms-publish-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let prepared = vec![
            Prepared {
                file: "moon.png".into(),
                name: "The Moon".into(),
                side: 5,
                png: b"moon".to_vec(),
            },
            Prepared {
                file: "earth.PNG".into(),
                name: "Earth".into(),
                side: 25,
                png: b"earth".to_vec(),
            },
        ];
        publish_local(&root, &prepared).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(MANIFEST)).unwrap(),
            "moon.png\tThe Moon\t5\nearth.PNG\tEarth\t25\n"
        );
        assert_eq!(fs::read(root.join("moon.png")).unwrap(), b"moon");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn device_transfer_is_atomic_at_the_manifest_boundary() {
        let prepared = vec![Prepared {
            file: "moon.png".into(),
            name: "Moon".into(),
            side: 5,
            png: b"moon".to_vec(),
        }];
        let script = transfer_script(&prepared);
        assert!(
            script.find("mv -f \"$stage/moon.png\"").unwrap()
                < script.find("mv -f \"$stage/imported.txt\"").unwrap()
        );
        assert!(script.contains("sha256sum"));
        assert!(script.contains("trap 'rm -rf"));
    }

    #[test]
    fn help_succeeds() {
        command(&["--help".into()]).unwrap();
    }
    fn grey_png(width: u32, height: u32, grey: &[u8]) -> Vec<u8> {
        let picture = Picture::from_grey(width, height, grey.to_vec()).unwrap();
        kobo_image::encode_png_grey(width, height, picture.grey()).unwrap()
    }
}
