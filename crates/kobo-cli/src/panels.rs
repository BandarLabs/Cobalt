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
            describe_large(&large_pages(&bytes, &comic)?);
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

/// Says which pages a push will scale down, so the owner hears it before
/// sending rather than finding a page the reader would not open.
fn describe_large(large: &[Large]) {
    if large.is_empty() {
        return;
    }
    println!(
        "Scaled down on push: {} page{} larger than the {} pixels the reader opens",
        large.len(),
        if large.len() == 1 { "" } else { "s" },
        kobo_image::MAX_PIXELS
    );
    for page in large {
        println!(
            "  page {}: {} ({} x {})",
            page.index + 1,
            page.name,
            page.width,
            page.height
        );
    }
}

/// A page bigger than the reader decodes.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Large {
    index: usize,
    name: String,
    width: u32,
    height: u32,
}

/// The pages of a comic the reader would refuse to decode, read from their
/// headers alone.
///
/// The reader's ceiling, [`kobo_image::MAX_PIXELS`], is right for a single
/// core with half a gigabyte, and a two-page spread from a digital release is
/// often past it. Such a page used to arrive and then fail on the reader with
/// "This page could not be opened", with nothing said on the computer.
fn large_pages(bytes: &[u8], comic: &kobo_comic::Comic) -> Result<Vec<Large>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("read the comic: {e}"))?;
    let mut large = Vec::new();
    for (index, name) in comic.pages.iter().enumerate() {
        let encoded = entry_bytes(&mut archive, name)?;
        let Ok((width, height)) = image::ImageReader::new(std::io::Cursor::new(&encoded))
            .with_guessed_format()
            .map_err(|_| ())
            .and_then(|reader| reader.into_dimensions().map_err(|_| ()))
        else {
            // A page that is not a picture at all is the reader's to report,
            // as it always has; scaling cannot help it.
            continue;
        };
        if u64::from(width) * u64::from(height) > kobo_image::MAX_PIXELS {
            large.push(Large {
                index,
                name: name.clone(),
                width,
                height,
            });
        }
    }
    Ok(large)
}

fn entry_bytes(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut file = archive
        .by_name(name)
        .map_err(|e| format!("read {name} from the comic: {e}"))?;
    let mut encoded = Vec::new();
    (&mut file)
        .take(kobo_image::MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut encoded)
        .map_err(|e| format!("read {name} from the comic: {e}"))?;
    Ok(encoded)
}

/// The largest size inside the reader's pixel budget with the page's shape.
fn within_budget(width: u32, height: u32) -> (u32, u32) {
    // Across times down is the budget when across squared is the budget
    // times width over height; the square root of that, in whole pixels.
    let widest = (kobo_image::MAX_PIXELS * u64::from(width) / u64::from(height.max(1))).isqrt();
    let mut across = u32::try_from(widest).unwrap_or(width).clamp(1, width);
    let down = |across: u32| {
        u32::try_from(u64::from(height) * u64::from(across) / u64::from(width))
            .unwrap_or(height)
            .max(1)
    };
    while across > 1 && u64::from(across) * u64::from(down(across)) > kobo_image::MAX_PIXELS {
        across -= 1;
    }
    (across, down(across))
}

/// One large page, scaled into the reader's budget and written back in the
/// format it came in, colour and all: a colour reader shows colour pages.
fn scaled_page(name: &str, encoded: &[u8]) -> Result<Vec<u8>, String> {
    let failed = |e: image::ImageError| format!("scale {name}: {e}");
    let reader = image::ImageReader::new(std::io::Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|e| format!("scale {name}: {e}"))?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(failed)?;
    let orientation = image::ImageDecoder::orientation(&mut decoder).map_err(failed)?;
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(failed)?;
    // Turned upright here, because the scaled copy carries no orientation
    // tag for the reader to turn it by.
    picture.apply_orientation(orientation);
    let (width, height) = within_budget(picture.width(), picture.height());
    let scaled = picture.resize_exact(width, height, image::imageops::FilterType::Lanczos3);
    let color = scaled.color();
    let mut out = Vec::new();
    if format == Some(image::ImageFormat::Jpeg) {
        let flat = if color.has_color() {
            image::DynamicImage::ImageRgb8(scaled.to_rgb8())
        } else {
            image::DynamicImage::ImageLuma8(scaled.to_luma8())
        };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&flat)
            .map_err(failed)?;
    } else {
        let eight_bit = match (color.has_color(), color.has_alpha()) {
            (true, true) => image::DynamicImage::ImageRgba8(scaled.to_rgba8()),
            (true, false) => image::DynamicImage::ImageRgb8(scaled.to_rgb8()),
            (false, true) => image::DynamicImage::ImageLumaA8(scaled.to_luma_alpha8()),
            (false, false) => image::DynamicImage::ImageLuma8(scaled.to_luma8()),
        };
        eight_bit
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .map_err(failed)?;
    }
    if out.len() > kobo_image::MAX_SOURCE_BYTES {
        return Err(format!(
            "{name} is still over Panels' 4 MiB page limit once scaled to fit the reader; export it smaller"
        ));
    }
    Ok(out)
}

/// The comic as it should travel: unchanged when every page fits the reader,
/// otherwise the same archive with only the large pages scaled down.
///
/// Every other entry is copied byte for byte, `ComicInfo.xml` and all, and the
/// scaled pages keep their names and places, so page order, the cover, the
/// title and the reading direction are what the owner made them.
fn fit_for_reader(bytes: Vec<u8>, large: &[Large]) -> Result<Vec<u8>, String> {
    if large.is_empty() {
        return Ok(bytes);
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice()))
        .map_err(|e| format!("read the comic: {e}"))?;
    let mut scaled = std::collections::BTreeMap::new();
    for page in large {
        let encoded = entry_bytes(&mut archive, &page.name)?;
        scaled.insert(page.name.clone(), scaled_page(&page.name, &encoded)?);
    }
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(|e| format!("read the comic: {e}"))?;
        if let Some(page) = scaled.get(file.name()) {
            let mut options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            if let Some(modified) = file.last_modified() {
                options = options.last_modified_time(modified);
            }
            let name = file.name().to_owned();
            drop(file);
            writer
                .start_file(name, options)
                .map_err(|e| format!("write the comic: {e}"))?;
            std::io::Write::write_all(&mut writer, page)
                .map_err(|e| format!("write the comic: {e}"))?;
        } else {
            writer
                .raw_copy_file(file)
                .map_err(|e| format!("write the comic: {e}"))?;
        }
    }
    Ok(writer
        .finish()
        .map_err(|e| format!("write the comic: {e}"))?
        .into_inner())
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
    /// How many pages were scaled down to fit the reader.
    scaled: usize,
}

fn prepare(input: &str) -> Result<Sending, String> {
    let bytes = read(Path::new(input))?;
    let (comic, _) = kobo_comic::inspect_named(&bytes, input).map_err(|e| e.to_string())?;
    let large = large_pages(&bytes, &comic)?;
    let bytes = fit_for_reader(bytes, &large)?;
    if !large.is_empty() {
        // The rewritten archive is held to the reader's own checks, and to
        // the comic it came from, before anything is sent.
        let (fitted, _) = kobo_comic::inspect_named(&bytes, input).map_err(|e| e.to_string())?;
        if fitted != comic || !large_pages(&bytes, &fitted)?.is_empty() {
            return Err(format!(
                "{input} could not be scaled to fit the reader without changing it"
            ));
        }
    }
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
        scaled: large.len(),
    })
}

/// The line a push adds when it had to scale pages for the reader.
fn scaled_note(sending: &Sending) -> Option<String> {
    (sending.scaled > 0).then(|| {
        format!(
            "Scaled {} large page{} of {} down to fit the reader.",
            sending.scaled,
            if sending.scaled == 1 { "" } else { "s" },
            sending.sent.title
        )
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
        if let Some(note) = scaled_note(&sending) {
            println!("{note}");
        }
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
    for item in &sending {
        if let Some(note) = scaled_note(item) {
            println!("{note}");
        }
    }
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

    fn picture(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let page = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, 120])
        });
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(page)
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    /// A right-to-left comic whose middle page is a 3200 by 2400 spread,
    /// past what the reader decodes, between two pages it opens as they are.
    fn comic_with_a_spread() -> Vec<u8> {
        use std::io::Write as _;
        let entries: [(&str, Vec<u8>); 4] = [
            (
                "ComicInfo.xml",
                b"<ComicInfo><Title>Harbour Lights</Title><Manga>YesAndRightToLeft</Manga></ComicInfo>"
                    .to_vec(),
            ),
            ("pages/01.png", picture(800, 1200, image::ImageFormat::Png)),
            ("pages/02.jpg", picture(3200, 2400, image::ImageFormat::Jpeg)),
            ("pages/10.png", picture(800, 1200, image::ImageFormat::Png)),
        ];
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in &entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn inspect_names_the_pages_a_push_will_scale_to_fit_the_reader() {
        let bytes = comic_with_a_spread();
        let comic = kobo_comic::inspect(&bytes).unwrap();
        assert_eq!(
            large_pages(&bytes, &comic).unwrap(),
            [Large {
                index: 1,
                name: "pages/02.jpg".to_owned(),
                width: 3200,
                height: 2400,
            }]
        );
    }

    #[test]
    fn a_push_sends_the_scaled_comic_and_names_it_by_what_was_sent() {
        let path = std::env::temp_dir().join(format!("panels-spread-{}.cbz", std::process::id()));
        fs::write(&path, comic_with_a_spread()).unwrap();
        let sending = prepare(path.to_str().unwrap()).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(sending.scaled, 1);
        assert_eq!(sending.sent.title, "Harbour Lights");
        assert_eq!(sending.sent.pages, 3);
        assert_eq!(
            sending.sent.sha256,
            kobo_net::sha256::hex_digest(&sending.bytes),
            "the reader checks the bytes it receives, not the ones on the computer"
        );
        assert_eq!(
            scaled_note(&sending).as_deref(),
            Some("Scaled 1 large page of Harbour Lights down to fit the reader.")
        );
    }

    #[test]
    fn a_pushed_spread_too_big_for_the_reader_arrives_scaled_and_opens() {
        let original = comic_with_a_spread();
        let comic = kobo_comic::inspect(&original).unwrap();
        assert!(
            kobo_comic::page(&original, &comic, 1).is_err(),
            "the reader refuses the spread as it was made"
        );
        let large = large_pages(&original, &comic).unwrap();
        let fitted = fit_for_reader(original.clone(), &large).unwrap();
        let after = kobo_comic::inspect(&fitted).unwrap();
        assert_eq!(after, comic, "page order, names, title and direction stay");
        assert_eq!(after.metadata.right_to_left, Some(true));
        for index in 0..comic.pages.len() {
            kobo_comic::page(&fitted, &after, index).expect("every page opens on the reader");
        }
        let spread = kobo_comic::page(&fitted, &after, 1).unwrap();
        assert!(u64::from(spread.width()) * u64::from(spread.height()) <= kobo_image::MAX_PIXELS);
        assert_eq!(
            (spread.width(), spread.height()),
            (2877, 2157),
            "the spread keeps its shape"
        );
        // The pages that already fit travel byte for byte.
        let mut before = zip::ZipArchive::new(std::io::Cursor::new(original.as_slice())).unwrap();
        let mut now = zip::ZipArchive::new(std::io::Cursor::new(fitted.as_slice())).unwrap();
        for name in ["ComicInfo.xml", "pages/01.png", "pages/10.png"] {
            assert_eq!(
                entry_bytes(&mut before, name).unwrap(),
                entry_bytes(&mut now, name).unwrap()
            );
        }
    }

    #[test]
    fn a_comic_whose_pages_all_fit_is_sent_exactly_as_it_was() {
        let bytes = comic_with_a_spread();
        assert_eq!(fit_for_reader(bytes.clone(), &[]).unwrap(), bytes);
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
