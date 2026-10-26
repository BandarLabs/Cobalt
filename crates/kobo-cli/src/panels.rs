//! Inspect, preview and publish CBZ comics for Panels.

use std::fs;
use std::path::Path;

const MAX_BYTES: usize = kobo_comic::MAX_ARCHIVE_BYTES;
const BLOB: &str = "volume.cbz";
const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/data/panels";
const USAGE: &str = "usage: kobo panels inspect COMIC.cbz\n\
                     \x20      kobo panels preview COMIC.cbz --out DIRECTORY\n\
                     \x20      kobo panels push COMIC.cbz (--sim | --device IP | --out FILE)\n\
                     CBR/RAR is not supported; convert it to CBZ without changing the page images.";

#[derive(Clone, Copy)]
enum Destination<'a> {
    Simulator,
    Device(&'a str),
    File(&'a str),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    let verb = arguments.first().ok_or_else(|| USAGE.to_owned())?;
    let input = arguments.get(1).ok_or_else(|| USAGE.to_owned())?;
    let bytes = read(Path::new(input))?;
    let (comic, notice) = kobo_comic::inspect_named(&bytes, input).map_err(|e| e.to_string())?;
    match verb.as_str() {
        "inspect" if arguments.len() == 2 => {
            describe(&comic, notice.as_deref());
            Ok(())
        }
        "preview" => {
            let out = one_output(&arguments[2..])?;
            preview(Path::new(out), &bytes, &comic, notice.as_deref())
        }
        "push" => {
            let destination = destination(&arguments[2..])?;
            publish(destination, &bytes)?;
            println!(
                "Panels comic ready: {} · {} pages",
                title(&comic, input),
                comic.pages.len()
            );
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err("the comic exceeds Panels' 32 MiB archive limit".into());
    }
    fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn describe(comic: &kobo_comic::Comic, notice: Option<&str>) {
    println!(
        "Title: {}\nPages: {}\nCover: page {}\nDirection: {}",
        comic.metadata.title.as_deref().unwrap_or("Untitled comic"),
        comic.pages.len(),
        comic.metadata.cover.unwrap_or(0) + 1,
        if comic.metadata.right_to_left == Some(true) {
            "right to left"
        } else {
            "left to right"
        }
    );
    if let Some(notice) = notice {
        println!("Notice: {notice}");
    }
}

fn title<'a>(comic: &'a kobo_comic::Comic, input: &'a str) -> &'a str {
    comic
        .metadata
        .title
        .as_deref()
        .or_else(|| Path::new(input).file_stem().and_then(|s| s.to_str()))
        .unwrap_or("Comic")
}

fn one_output(arguments: &[String]) -> Result<&str, String> {
    match arguments {
        [flag, path] if flag == "--out" => Ok(path),
        _ => Err(USAGE.into()),
    }
}
fn destination(arguments: &[String]) -> Result<Destination<'_>, String> {
    match arguments {
        [flag] if flag == "--sim" => Ok(Destination::Simulator),
        [flag, path] if flag == "--out" => Ok(Destination::File(path)),
        [flag, host] if super::is_device_flag(flag) && super::valid_device_host(host) => {
            Ok(Destination::Device(host))
        }
        _ => Err(USAGE.into()),
    }
}

fn preview(
    root: &Path,
    bytes: &[u8],
    comic: &kobo_comic::Comic,
    notice: Option<&str>,
) -> Result<(), String> {
    if root.exists() {
        return Err(format!(
            "preview directory {} already exists",
            root.display()
        ));
    }
    fs::create_dir(root).map_err(|e| format!("create preview: {e}"))?;
    let first = kobo_comic::page(bytes, comic, comic.metadata.cover.unwrap_or(0))
        .map_err(|e| e.to_string())?;
    let png = kobo_image::encode_png_grey(first.width(), first.height(), first.grey())
        .map_err(|e| e.to_string())?;
    fs::write(root.join("cover.png"), png).map_err(|e| format!("write cover: {e}"))?;
    let escaped = |v: &str| {
        v.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let notice = notice.map_or(String::new(), |n| format!("<p>{}</p>", escaped(n)));
    let html=format!("<!doctype html><html><meta charset=utf-8><meta name=viewport content='width=device-width'><style>body{{font:18px/1.5 system-ui;max-width:760px;margin:auto;padding:24px}}img{{max-width:100%;max-height:70vh}}</style><h1>{}</h1><p>{} pages · {} · cover page {}</p>{}<img src=cover.png alt='Comic cover preview'>",escaped(comic.metadata.title.as_deref().unwrap_or("Comic preview")),comic.pages.len(),if comic.metadata.right_to_left==Some(true){"right to left"}else{"left to right"},comic.metadata.cover.unwrap_or(0)+1,notice);
    fs::write(root.join("index.html"), html).map_err(|e| format!("write preview: {e}"))?;
    println!(
        "Prepared Panels preview: {}",
        root.join("index.html").display()
    );
    Ok(())
}

fn publish(destination: Destination<'_>, bytes: &[u8]) -> Result<(), String> {
    match destination {
        Destination::Simulator => {
            atomic(&kobo_sim::simulated_data_root("panels").join(BLOB), bytes)
        }
        Destination::File(path) => atomic(Path::new(path), bytes),
        Destination::Device(host) => {
            let encoded = super::base64_encode(bytes);
            let count = bytes.len();
            let digest = kobo_net::sha256::hex_digest(bytes);
            let script=format!("set -eu\nroot={DEVICE_ROOT}\nmkdir -p \"$root\"\npartial=\"$root/.{BLOB}.$$.writing\"\ntrap 'rm -f \"$partial\"' EXIT HUP INT TERM\nbase64 -d > \"$partial\" <<'KOBO_PANELS_CBZ'\n{encoded}\nKOBO_PANELS_CBZ\ntest \"$(wc -c < \"$partial\")\" = '{count}'\nset -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{BLOB}\"\nsync\n");
            let out = super::run_remote_shell(
                &format!("root@{host}"),
                &script,
                super::REMOTE_COMMAND_TIMEOUT,
            )
            .map_err(super::unreachable_device)?;
            if !out.status.success() {
                return Err(format!(
                    "the reader refused the Panels transfer: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Ok(())
        }
    }
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("output needs a parent directory")?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let partial = parent.join(format!(
        ".{}.$$.writing",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("comic")
    ));
    fs::write(&partial, bytes).map_err(|e| format!("write staging file: {e}"))?;
    fs::rename(&partial, path).map_err(|e| format!("publish {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn help_succeeds() {
        command(&["--help".into()]).unwrap();
    }
    #[test]
    fn genuine_rar_has_clear_guidance() {
        let e = kobo_comic::inspect_named(b"Rar!\x1a\x07\0", "book.cbz")
            .unwrap_err()
            .to_string();
        assert!(e.contains("CBR"));
        assert!(e.contains("CBZ"));
    }
}
