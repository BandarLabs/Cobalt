//! Safe host-side preparation and transfer for Nonograms photo puzzles.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/nonograms";
const BLOB: &str = "photo.png";
const NORMALIZED_EDGE: u32 = 360;
const MAX_TRANSFER_BYTES: usize = 256 * 1024;
const PLAYABLE_SIDES: [usize; 3] = [5, 7, 9];
const USAGE: &str =
    "usage: kobo nonograms push IMAGE --size N (--device IP | --out photo.png)\nN is exactly 5, 7, or 9.";

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
    let push = parse_push(arguments)?;
    let source = read_image(Path::new(push.input))?;
    let png = prepare(&source, push.side)?;
    match push.destination {
        Destination::Device(host) => transfer(&png, host),
        Destination::Output(path) => {
            write_output(Path::new(path), &png)?;
            println!("Prepared Nonograms photo: {path}");
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
        parse_push, prepare, size_error, transfer_script, Destination, Push, BLOB, NORMALIZED_EDGE,
    };
    use kobo_image::Picture;

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
        let error = super::super::run(&["nonograms".to_owned(), "look".to_owned()]).expect_err("usage");
        assert!(error.starts_with("usage: kobo nonograms push"));
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
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
    }

    fn grey_png(width: u32, height: u32, grey: &[u8]) -> Vec<u8> {
        let picture = Picture::from_grey(width, height, grey.to_vec()).expect("picture");
        kobo_image::encode_png_grey(picture.width(), picture.height(), picture.grey()).expect("png")
    }
}
