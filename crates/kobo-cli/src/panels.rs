//! Inspect, preview and publish CBZ comics for Panels.

use kobo_comic::inbox::{self, Sent};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const MAX_BYTES: usize = kobo_comic::MAX_ARCHIVE_BYTES;
const BLOB: &str = "volume.cbz";
const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/data/panels";
const USAGE: &str = concat!(
    "usage: kobo panels inspect COMIC.cbz\n\
                     \x20      kobo panels preview COMIC.cbz --out DIRECTORY\n\
                     \x20      kobo panels push COMIC.cbz... [--replace] ",
    "[", crate::targets::target_flags!(), " | --out FILE]",
    "\n\
                     A push adds the comics to the ones waiting in Panels, each under its\n\
                     own title, and Panels puts them on its shelf when you next open it or\n\
                     come back to it. --replace drops the others still waiting. --out\n\
                     writes one comic to a file to copy over USB.\n\
                     CBR/RAR is not supported; convert it to CBZ without changing the page images.\n",
    crate::targets::saved_reader_note!(),
);

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
    if arguments.first().map(String::as_str) == Some("push") {
        return push(&arguments[1..]);
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

/// One comic read, checked and named, ready to send.
struct Sending {
    bytes: Vec<u8>,
    sent: Sent,
}

fn prepare(input: &str) -> Result<Sending, String> {
    let bytes = read(Path::new(input))?;
    let (comic, _) = kobo_comic::inspect_named(&bytes, input).map_err(|e| e.to_string())?;
    let sha256 = kobo_net::sha256::hex_digest(&bytes);
    // The title travels in the list the app reads, so it is cut to what the
    // app accepts there rather than refused for being long.
    let mut title: String = title(&comic, input)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    while title.len() > 256 {
        title.pop();
    }
    if title.trim().is_empty() {
        "Comic".clone_into(&mut title);
    }
    Ok(Sending {
        sent: Sent {
            file: inbox::file_name(&sha256),
            title,
            pages: comic.pages.len(),
            sha256,
        },
        bytes,
    })
}

/// The comics that will be waiting after this push, and the files of those
/// that no longer are.
///
/// A comic Panels has already added is gone from the shelf, because the app
/// deletes its sent file once its own copy is safe, so it drops out of the
/// list here. A comic sent again under the same title replaces the copy
/// still waiting.
fn merge(
    waiting: Vec<Sent>,
    present: &dyn Fn(&str) -> bool,
    sending: &[Sent],
    replace: bool,
) -> Result<(Vec<Sent>, Vec<String>), String> {
    let mut list: Vec<Sent> = Vec::new();
    let mut dropped = Vec::new();
    for old in waiting {
        let resent = sending
            .iter()
            .any(|new| new.sha256 == old.sha256 || new.title == old.title);
        if !present(&old.file) {
            continue;
        }
        if replace || resent {
            if !sending.iter().any(|new| new.file == old.file) {
                dropped.push(old.file.clone());
            }
            continue;
        }
        list.push(old);
    }
    for new in sending {
        if !list.iter().any(|kept| kept.sha256 == new.sha256) {
            list.push(new.clone());
        }
    }
    if list.len() > inbox::MAX_WAITING {
        return Err(format!(
            "Panels holds {} comics waiting to be added. Add the ones already waiting in Panels first, or push with --replace.",
            inbox::MAX_WAITING
        ));
    }
    Ok((list, dropped))
}

fn waiting_summary(list: &[Sent]) -> String {
    let mut text = format!(
        "Waiting in Panels: {} comic{}.\n",
        list.len(),
        if list.len() == 1 { "" } else { "s" }
    );
    for sent in list {
        let plural = if sent.pages == 1 { "" } else { "s" };
        let _ = writeln!(text, "  {} ({} page{plural})", sent.title, sent.pages);
    }
    text.push_str("Panels adds them to its shelf the next time you open it or come back to it.\n");
    text
}

fn push(arguments: &[String]) -> Result<(), String> {
    let mut inputs = Vec::new();
    let mut flags = Vec::new();
    let mut replace = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--replace" {
            replace = true;
        } else if argument.starts_with('-') {
            flags.push(argument.clone());
            if let Some(value) = arguments.get(index + 1).filter(|_| argument != "--sim") {
                flags.push(value.clone());
                index += 1;
            }
        } else {
            inputs.push(argument.as_str());
        }
        index += 1;
    }
    if inputs.is_empty() {
        return Err(USAGE.to_owned());
    }
    let destination = destination(&flags)?;
    if let Destination::File(path) = destination {
        let [input] = inputs.as_slice() else {
            return Err("--out writes one comic to one file; name a single comic.".to_owned());
        };
        let sending = prepare(input)?;
        atomic(Path::new(path), &sending.bytes)?;
        println!(
            "Wrote {} ({} pages) to {path}. Copy it to .adds/cobalt/data/panels/{BLOB} on the reader.",
            sending.sent.title, sending.sent.pages
        );
        return Ok(());
    }
    let sending = inputs
        .iter()
        .map(|input| prepare(input))
        .collect::<Result<Vec<_>, _>>()?;
    let offered = sending
        .iter()
        .map(|item| item.sent.clone())
        .collect::<Vec<_>>();
    let list = match destination {
        Destination::Simulator => send_to_simulator(&sending, &offered, replace)?,
        Destination::Device(host) => send_to_reader(host, &sending, &offered, replace)?,
        Destination::File(_) => unreachable!("handled above"),
    };
    for item in &offered {
        println!("Sent {} ({} pages).", item.title, item.pages);
    }
    print!("{}", waiting_summary(&list));
    Ok(())
}

fn send_to_simulator(
    sending: &[Sending],
    offered: &[Sent],
    replace: bool,
) -> Result<Vec<Sent>, String> {
    let root = kobo_sim::simulated_data_root("panels");
    let waiting = match fs::read(root.join(inbox::MANIFEST)) {
        Ok(bytes) => inbox::decode(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("read the Panels list in the simulator: {error}")),
    };
    let present = |file: &str| root.join(file).is_file();
    let (list, dropped) = merge(waiting, &present, offered, replace)?;
    for item in sending {
        atomic(&root.join(&item.sent.file), &item.bytes)?;
    }
    atomic(&root.join(inbox::MANIFEST), &inbox::encode(&list))?;
    for file in dropped {
        let _ = fs::remove_file(root.join(file));
    }
    Ok(list)
}

fn remote(host: &str, script: &str) -> Result<Vec<u8>, String> {
    let out = super::run_remote_shell(
        &format!("root@{host}"),
        script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !out.status.success() {
        return Err(format!(
            "the reader refused the Panels transfer: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn send_to_reader(
    host: &str,
    sending: &[Sending],
    offered: &[Sent],
    replace: bool,
) -> Result<Vec<Sent>, String> {
    let manifest = inbox::MANIFEST;
    let listing = remote(
        host,
        &format!(
            "set -eu\nroot={DEVICE_ROOT}\nmkdir -p \"$root\"\nfor f in \"$root\"/sent-*.cbz; do [ -f \"$f\" ] && echo \"file ${{f##*/}}\"; done; true\nif [ -f \"$root/{manifest}\" ]; then printf 'list '; base64 \"$root/{manifest}\" | tr -d '\\n'; echo; fi\n"
        ),
    )?;
    let listing = String::from_utf8_lossy(&listing);
    let files: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.strip_prefix("file "))
        .collect();
    let waiting = match listing.lines().find_map(|line| line.strip_prefix("list ")) {
        Some(encoded) => inbox::decode(&super::frame::base64_decode(encoded.trim())?)?,
        None => Vec::new(),
    };
    let present = |file: &str| files.contains(&file);
    let (list, dropped) = merge(waiting, &present, offered, replace)?;
    for item in sending {
        let encoded = super::base64_encode(&item.bytes);
        let count = item.bytes.len();
        let (file, digest) = (&item.sent.file, &item.sent.sha256);
        remote(
            host,
            &format!("set -eu\nroot={DEVICE_ROOT}\npartial=\"$root/.{file}.$$.writing\"\ntrap 'rm -f \"$partial\"' EXIT HUP INT TERM\nbase64 -d > \"$partial\" <<'KOBO_PANELS_CBZ'\n{encoded}\nKOBO_PANELS_CBZ\ntest \"$(wc -c < \"$partial\")\" = '{count}'\nset -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{file}\"\nsync\n"),
        )?;
    }
    let encoded = super::base64_encode(&inbox::encode(&list));
    let mut script = format!("set -eu\nroot={DEVICE_ROOT}\npartial=\"$root/.{manifest}.$$.writing\"\nbase64 -d > \"$partial\" <<'KOBO_PANELS_LIST'\n{encoded}\nKOBO_PANELS_LIST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{manifest}\"\n");
    for file in dropped {
        let _ = writeln!(script, "rm -f \"$root/{file}\"");
    }
    script.push_str("sync\n");
    remote(host, &script)?;
    Ok(list)
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
    fn sent(title: &str, digit: char) -> Sent {
        let sha256 = digit.to_string().repeat(64);
        Sent {
            file: inbox::file_name(&sha256),
            title: title.to_owned(),
            pages: 4,
            sha256,
        }
    }

    #[test]
    fn a_second_push_keeps_the_first_comic_waiting() {
        let first = sent("A small garden", 'a');
        let second = sent("The lighthouse", 'b');
        let (list, dropped) =
            merge(vec![first.clone()], &|_| true, &[second.clone()], false).unwrap();
        assert_eq!(list, [first, second]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn replace_drops_the_others_still_waiting_and_their_files() {
        let first = sent("A small garden", 'a');
        let second = sent("The lighthouse", 'b');
        let (list, dropped) =
            merge(vec![first.clone()], &|_| true, &[second.clone()], true).unwrap();
        assert_eq!(list, [second]);
        assert_eq!(dropped, [first.file]);
    }

    #[test]
    fn a_comic_panels_already_added_leaves_the_list() {
        let added = sent("A small garden", 'a');
        let (list, dropped) = merge(vec![added], &|_| false, &[], false).unwrap();
        assert!(list.is_empty());
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_comic_sent_again_under_its_title_replaces_the_copy_still_waiting() {
        let old = sent("A small garden", 'a');
        let new = sent("A small garden", 'c');
        let (list, dropped) = merge(vec![old.clone()], &|_| true, &[new.clone()], false).unwrap();
        assert_eq!(list, [new]);
        assert_eq!(dropped, [old.file]);
    }

    #[test]
    fn the_waiting_list_is_bounded_and_says_how_to_make_room() {
        let full = (0..inbox::MAX_WAITING)
            .map(|index| sent(&format!("Comic {index}"), 'a'))
            .enumerate()
            .map(|(index, mut item)| {
                item.sha256 = format!("{index:064x}");
                item.file = inbox::file_name(&item.sha256);
                item
            })
            .collect::<Vec<_>>();
        let error = merge(full, &|_| true, &[sent("One more", 'f')], false).unwrap_err();
        assert!(error.contains("--replace"));
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
