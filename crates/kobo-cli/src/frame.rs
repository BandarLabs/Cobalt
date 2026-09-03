//! Owner-attended Frame shelf management over the already-paired SSH channel.

use kobo_frame_host::{
    prepare_for_panel, Fit, Manifest, Panel, Push, MANIFEST, MAX_FRAME_CAPACITY,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/frame";
const KOBOD: &str = "/mnt/onboard/.adds/cobalt/bin/kobod";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const USAGE: &str = "usage: kobo frame init --device IP\n\
                     \x20      kobo frame push INPUT --device IP [--fit crop|pad] [--delete]\n\
                     \x20      kobo frame ls --device IP\n\
                     \x20      kobo frame rm ID --device IP";

pub fn command(arguments: &[String]) -> Result<(), String> {
    match arguments.first().map(String::as_str) {
        Some("init") => init(&arguments[1..]),
        Some("push") => push(&arguments[1..]),
        Some("ls") => list(&arguments[1..]),
        Some("rm") => remove(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    let host = device_only(arguments)?;
    let output = remote(
        host,
        &format!(
            "set -eu\nmkdir -p '{ROOT}'\nchmod 700 '{ROOT}'\nsync\nprintf 'Frame shelf ready; transfers use this owner-attended SSH connection.\\n'\n"
        ),
    )?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn push(arguments: &[String]) -> Result<(), String> {
    let (input, host, fit, delete) = parse_push(arguments)?;
    let panel = reader_panel(host)?;
    let existing = read_manifest(host)?;
    let push = prepare_for_panel(Path::new(input), fit, &existing, delete, panel)?;
    enforce_capacity(host, &push)?;
    transfer(host, &push)?;
    println!(
        "Frame updated: {} photo(s), {} new, {} removed",
        push.manifest.photos.len(),
        push.photos
            .iter()
            .filter(|photo| photo.png.is_some())
            .count(),
        push.removed.len()
    );
    Ok(())
}

fn reader_panel(host: &str) -> Result<Panel, String> {
    let output = remote(
        host,
        &format!("set -eu\n'{KOBOD}' | sed -n 's/^profile: //p'\n"),
    )?;
    let profile_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let profile = kobo_profile::SUPPORTED_PROFILES
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("Frame cannot identify the reader profile {profile_id:?}"))?;
    Ok(Panel {
        width: profile.width,
        height: profile.height,
    })
}

fn list(arguments: &[String]) -> Result<(), String> {
    let host = device_only(arguments)?;
    let manifest = read_manifest(host)?;
    if manifest.photos.is_empty() {
        println!("Frame shelf is empty.");
        return Ok(());
    }
    for photo in manifest.photos {
        println!(
            "{}\t{}\t{}\t{}",
            photo.id, photo.taken, photo.album, photo.name
        );
    }
    Ok(())
}

fn remove(arguments: &[String]) -> Result<(), String> {
    let [id, flag, host] = arguments else {
        return Err(USAGE.to_owned());
    };
    if !super::is_device_flag(flag) || !super::valid_device_host(host) {
        return Err(USAGE.to_owned());
    }
    let mut manifest = read_manifest(host)?;
    let Some(index) = manifest.photos.iter().position(|photo| photo.id == *id) else {
        return Err(format!("Frame has no photo with id {id:?}"));
    };
    let photo = manifest.photos.remove(index);
    let encoded = super::base64_encode(&manifest.encode());
    let script = format!(
        "set -eu\nroot='{ROOT}'\npartial=\"$root/.{MANIFEST}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_FRAME_MANIFEST'\n{encoded}\nCOBALT_FRAME_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{MANIFEST}\"\nsync\nrm -f \"$root/{}.png\"\nsync\nprintf 'Removed Frame photo {}\\n'\n",
        photo.id, photo.id
    );
    let output = remote(host, &script)?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn parse_push(arguments: &[String]) -> Result<(&str, &str, Fit, bool), String> {
    let Some(input) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    let mut host = None;
    let mut fit = Fit::Crop;
    let mut delete = false;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            flag if super::is_device_flag(flag) => {
                host = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            "--fit" => {
                fit = Fit::parse(arguments.get(index + 1).ok_or_else(|| USAGE.to_owned())?)?;
                index += 2;
            }
            "--delete" => {
                delete = true;
                index += 1;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let host = host.ok_or_else(|| USAGE.to_owned())?;
    if !super::valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    Ok((input, host, fit, delete))
}

fn device_only(arguments: &[String]) -> Result<&str, String> {
    let [flag, host] = arguments else {
        return Err(USAGE.to_owned());
    };
    if !super::is_device_flag(flag) || !super::valid_device_host(host) {
        return Err(USAGE.to_owned());
    }
    Ok(host)
}

fn read_manifest(host: &str) -> Result<Manifest, String> {
    let output = remote(
        host,
        &format!("set -eu\nif [ -f '{ROOT}/{MANIFEST}' ]; then base64 '{ROOT}/{MANIFEST}'; fi\n"),
    )?;
    if output.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Manifest::default());
    }
    let bytes = base64_decode(&String::from_utf8_lossy(&output.stdout))?;
    Manifest::decode(&bytes)
        .map_err(|error| format!("the reader's Frame manifest is invalid: {error}"))
}

fn enforce_capacity(host: &str, push: &Push) -> Result<(), String> {
    let incoming = push
        .photos
        .iter()
        .filter(|prepared| prepared.png.is_some())
        .map(|prepared| prepared.photo.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let retained = push
        .manifest
        .photos
        .iter()
        .filter(|photo| !incoming.contains(photo.id.as_str()))
        .map(|photo| photo.id.as_str())
        .collect::<Vec<_>>();
    let current = frame_sizes(host, &retained)?;
    let total = capacity_bytes(push, &current)?;
    if total > MAX_FRAME_CAPACITY {
        return Err(format!(
            "this would use {} MB for Frame photos; its capacity is {} MB",
            total / (1024 * 1024),
            MAX_FRAME_CAPACITY / (1024 * 1024)
        ));
    }
    Ok(())
}

fn capacity_bytes(push: &Push, current: &BTreeMap<String, usize>) -> Result<usize, String> {
    let incoming = push
        .photos
        .iter()
        .filter(|prepared| prepared.png.is_some())
        .map(|prepared| prepared.photo.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut retained = push
        .manifest
        .photos
        .iter()
        .filter(|photo| !incoming.contains(photo.id.as_str()))
        .map(|photo| photo.id.as_str());
    let retained_bytes = retained.try_fold(0_usize, |total, id| {
        let bytes = current
            .get(id)
            .ok_or_else(|| format!("Frame photo {id} is missing from the reader"))?;
        total
            .checked_add(*bytes)
            .ok_or_else(|| "Frame capacity calculation overflowed".to_owned())
    })?;
    let incoming_bytes = push
        .photos
        .iter()
        .filter_map(|prepared| prepared.png.as_ref())
        .try_fold(0_usize, |total, png| {
            total
                .checked_add(png.len())
                .ok_or_else(|| "Frame capacity calculation overflowed".to_owned())
        })?;
    let total = retained_bytes
        .checked_add(incoming_bytes)
        .ok_or_else(|| "Frame capacity calculation overflowed".to_owned())?;
    Ok(total)
}

fn frame_sizes(host: &str, ids: &[&str]) -> Result<BTreeMap<String, usize>, String> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let probes = ids
        .iter()
        .map(|id| format!("if [ -f \"$root/{id}.png\" ]; then printf '{id}\\t'; wc -c < \"$root/{id}.png\"; fi"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = remote(host, &format!("set -eu\nroot='{ROOT}'\n{probes}\n"))?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (id, bytes) = line.split_once('\t')?;
            Some((id.to_owned(), bytes.parse().ok()?))
        })
        .collect())
}

fn transfer(host: &str, push: &Push) -> Result<(), String> {
    let script = transfer_script(push);
    let _ = remote(host, &script)?;
    Ok(())
}

fn transfer_script(push: &Push) -> String {
    let mut script = format!("set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\n");
    for prepared in push.photos.iter().filter(|prepared| prepared.png.is_some()) {
        let png = prepared.png.as_ref().expect("filtered");
        let encoded = super::base64_encode(png);
        let _ = write!(
            script,
            "partial=\"$root/.{}.png.writing\"\nbase64 -d > \"$partial\" <<'COBALT_FRAME_PHOTO'\n{encoded}\nCOBALT_FRAME_PHOTO\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{}.png\"\n",
            prepared.photo.id, prepared.photo.id
        );
    }
    let encoded = super::base64_encode(&push.manifest.encode());
    let _ = write!(
        script,
        "partial=\"$root/.{MANIFEST}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_FRAME_MANIFEST'\n{encoded}\nCOBALT_FRAME_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{MANIFEST}\"\nsync\n"
    );
    for photo in &push.removed {
        let _ = writeln!(script, "rm -f \"$root/{}.png\"", photo.id);
    }
    script.push_str("sync\n");
    script
}

fn remote(host: &str, script: &str) -> Result<super::RemoteShellOutput, String> {
    let output = super::run_remote_shell(&format!("root@{host}"), script, TRANSFER_TIMEOUT)
        .map_err(super::unreachable_device)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(super::unreachable_if_ssh_gave_up(
            super::remote_shell_error(
                format!("Frame transfer on {host} exited with {}", output.status),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ))
    }
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let cleaned = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if cleaned.len() % 4 != 0 {
        return Err("the reader returned malformed base64 Frame data".to_owned());
    }
    let mut decoded = Vec::with_capacity(cleaned.len() / 4 * 3);
    for group in cleaned.chunks_exact(4) {
        let first = base64_value(group[0])?;
        let second = base64_value(group[1])?;
        let third = if group[2] == b'=' {
            None
        } else {
            Some(base64_value(group[2])?)
        };
        let fourth = if group[3] == b'=' {
            None
        } else {
            Some(base64_value(group[3])?)
        };
        if third.is_none() && fourth.is_some() {
            return Err("the reader returned malformed base64 Frame data".to_owned());
        }
        decoded.push((first << 2) | (second >> 4));
        if let Some(third) = third {
            decoded.push((second << 4) | (third >> 2));
            if let Some(fourth) = fourth {
                decoded.push((third << 6) | fourth);
            }
        }
    }
    Ok(decoded)
}

fn base64_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err("the reader returned malformed base64 Frame data".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_decode, capacity_bytes, parse_push, transfer_script, MANIFEST};
    use kobo_frame_host::{Fit, Manifest, Photo, PreparedPhoto, Push};
    use std::collections::BTreeMap;

    #[test]
    fn parses_explicit_deletion_and_fit() {
        let arguments = vec![
            "family".into(),
            "--fit".into(),
            "pad".into(),
            "--delete".into(),
            "--device".into(),
            "192.0.2.1".into(),
        ];
        assert_eq!(
            parse_push(&arguments).expect("parse"),
            ("family", "192.0.2.1", Fit::Pad, true)
        );
    }

    #[test]
    fn reads_base64_emitted_by_busybox() {
        assert_eq!(base64_decode("aGVsbG8=\n").expect("decode"), b"hello");
    }

    #[test]
    fn publishes_manifest_before_deleting_unreferenced_photos() {
        let push = Push {
            manifest: Manifest::default(),
            photos: Vec::new(),
            removed: vec![Photo {
                id: "photo-old".to_owned(),
                digest: "old".to_owned(),
                taken: 1,
                album: "Album".to_owned(),
                name: "Old".to_owned(),
            }],
        };
        let script = transfer_script(&push);
        let publish = script
            .find(&format!("mv -f \"$partial\" \"$root/{MANIFEST}\""))
            .expect("manifest publication");
        let deletion = script
            .find("rm -f \"$root/photo-old.png\"")
            .expect("old photo deletion");
        assert!(publish < deletion);
    }

    #[test]
    fn capacity_counts_retained_photos_not_present_in_the_new_input() {
        let old = photo("photo-old");
        let new = photo("photo-new");
        let push = Push {
            manifest: Manifest {
                photos: vec![old.clone(), new.clone()],
            },
            photos: vec![PreparedPhoto {
                photo: new,
                png: Some(vec![0_u8; 32]),
            }],
            removed: Vec::new(),
        };
        let current = BTreeMap::from([(old.id, 64)]);
        assert_eq!(capacity_bytes(&push, &current).expect("capacity"), 96);
    }

    fn photo(id: &str) -> Photo {
        Photo {
            id: id.to_owned(),
            digest: id.to_owned(),
            taken: 1,
            album: "Album".to_owned(),
            name: id.to_owned(),
        }
    }
}
