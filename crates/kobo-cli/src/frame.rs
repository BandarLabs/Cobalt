//! Owner-attended Frame shelf management over the already-paired SSH channel.

use kobo_frame_host::{
    prepare_for_panel, Fit, Manifest, Panel, Push, MANIFEST, MAX_FRAME_CAPACITY,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/frame";
const KOBOD: &str = "/mnt/onboard/.adds/cobalt/bin/kobod";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);
const USAGE: &str = "usage: kobo frame init (--sim | --device IP)\n\
                     \x20      kobo frame push INPUT (--sim | --device IP) [--fit crop|pad] [--delete]\n\
                     \x20      kobo frame ls (--sim | --device IP)\n\
                     \x20      kobo frame rm ID (--sim | --device IP)";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Device(String),
    Sim,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("init") => init(&arguments[1..]),
        Some("push") => push(&arguments[1..]),
        Some("ls") => list(&arguments[1..]),
        Some("rm") => remove(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    match parse_target(arguments)? {
        Target::Device(host) => {
            let output = remote(
                &host,
                &format!(
                    "set -eu\nmkdir -p '{ROOT}'\nchmod 700 '{ROOT}'\nsync\nprintf 'Frame shelf ready; transfers use this owner-attended SSH connection.\\n'\n"
                ),
            )?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        Target::Sim => {
            let root = sim_root();
            fs::create_dir_all(&root)
                .map_err(|error| format!("create Frame simulator shelf: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                    .map_err(|error| format!("protect Frame simulator shelf: {error}"))?;
            }
            println!(
                "Frame shelf ready at {}; transfers stay on this computer.",
                root.display()
            );
        }
    }
    Ok(())
}

fn push(arguments: &[String]) -> Result<(), String> {
    let (input, target, fit, delete) = parse_push(arguments)?;
    let (existing, panel) = match &target {
        Target::Device(host) => (read_manifest(host)?, reader_panel(host)?),
        Target::Sim => (read_local_manifest()?, sim_panel()),
    };
    let push = prepare_for_panel(Path::new(input), fit, &existing, delete, panel)?;
    match &target {
        Target::Device(host) => {
            enforce_capacity(host, &push)?;
            transfer(host, &push)?;
        }
        Target::Sim => {
            enforce_local_capacity(&push)?;
            publish_local(&push)?;
        }
    }
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
    let manifest = match parse_target(arguments)? {
        Target::Device(host) => read_manifest(&host)?,
        Target::Sim => read_local_manifest()?,
    };
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
    let (id, target) = parse_remove(arguments)?;
    match target {
        Target::Device(host) => {
            let mut manifest = read_manifest(&host)?;
            let Some(index) = manifest.photos.iter().position(|photo| photo.id == id) else {
                return Err(format!("Frame has no photo with id {id:?}"));
            };
            let photo = manifest.photos.remove(index);
            let encoded = super::base64_encode(&manifest.encode());
            let script = format!(
                "set -eu\nroot='{ROOT}'\npartial=\"$root/.{MANIFEST}.writing\"\nbase64 -d > \"$partial\" <<'COBALT_FRAME_MANIFEST'\n{encoded}\nCOBALT_FRAME_MANIFEST\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{MANIFEST}\"\nsync\nrm -f \"$root/{}.png\"\nsync\nprintf 'Removed Frame photo {}\\n'\n",
                photo.id, photo.id
            );
            let output = remote(&host, &script)?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        Target::Sim => {
            let mut manifest = read_local_manifest()?;
            let Some(index) = manifest.photos.iter().position(|photo| photo.id == id) else {
                return Err(format!("Frame has no photo with id {id:?}"));
            };
            let photo = manifest.photos.remove(index);
            publish_local(&Push {
                manifest,
                photos: Vec::new(),
                removed: vec![photo.clone()],
            })?;
            println!("Removed Frame photo {}", photo.id);
        }
    }
    Ok(())
}

fn parse_push(arguments: &[String]) -> Result<(&str, Target, Fit, bool), String> {
    let Some(input) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    let mut target = None;
    let mut fit = Fit::Crop;
    let mut delete = false;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            flag if super::is_device_flag(flag) => {
                if target.is_some() {
                    return Err(USAGE.to_owned());
                }
                let host = arguments
                    .get(index + 1)
                    .ok_or_else(|| USAGE.to_owned())?
                    .as_str();
                if !super::valid_device_host(host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                target = Some(Target::Device(host.to_owned()));
                index += 2;
            }
            "--sim" => {
                if target.is_some() {
                    return Err(USAGE.to_owned());
                }
                target = Some(Target::Sim);
                index += 1;
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
    let target = target.ok_or_else(|| USAGE.to_owned())?;
    Ok((input, target, fit, delete))
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
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_remove(arguments: &[String]) -> Result<(String, Target), String> {
    match arguments {
        [id, flag, host] if super::is_device_flag(flag) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Ok((id.clone(), Target::Device(host.clone())))
        }
        [id, flag] if flag == "--sim" => Ok((id.clone(), Target::Sim)),
        _ => Err(USAGE.to_owned()),
    }
}

fn sim_root() -> PathBuf {
    kobo_sim::simulated_data_root("frame")
}

fn sim_panel() -> Panel {
    let profile = kobo_sim::selected_profile();
    Panel {
        width: profile.width,
        height: profile.height,
    }
}

fn read_local_manifest() -> Result<Manifest, String> {
    let path = sim_root().join(MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("the simulator Frame manifest is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(error) => Err(format!("read Frame simulator shelf: {error}")),
    }
}

fn publish_local(push: &Push) -> Result<(), String> {
    let root = sim_root();
    fs::create_dir_all(&root).map_err(|error| format!("create Frame simulator shelf: {error}"))?;
    for prepared in push.photos.iter().filter(|prepared| prepared.png.is_some()) {
        let png = prepared.png.as_ref().expect("filtered");
        let dest = root.join(format!("{}.png", prepared.photo.id));
        let partial = root.join(format!(".{}.png.writing", prepared.photo.id));
        fs::write(&partial, png)
            .map_err(|error| format!("write Frame photo {}: {error}", prepared.photo.id))?;
        fs::rename(&partial, &dest)
            .map_err(|error| format!("publish Frame photo {}: {error}", prepared.photo.id))?;
    }
    let dest = root.join(MANIFEST);
    let partial = root.join(format!(".{MANIFEST}.writing"));
    fs::write(&partial, push.manifest.encode())
        .map_err(|error| format!("write Frame manifest: {error}"))?;
    fs::rename(&partial, &dest).map_err(|error| format!("publish Frame manifest: {error}"))?;
    for photo in &push.removed {
        let _ = fs::remove_file(root.join(format!("{}.png", photo.id)));
    }
    Ok(())
}

fn enforce_local_capacity(push: &Push) -> Result<(), String> {
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
    let current = local_sizes(&retained)?;
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

fn local_sizes(ids: &[&str]) -> Result<BTreeMap<String, usize>, String> {
    let root = sim_root();
    let mut sizes = BTreeMap::new();
    for id in ids {
        let path = root.join(format!("{id}.png"));
        match fs::metadata(&path) {
            Ok(metadata) => {
                sizes.insert(
                    (*id).to_owned(),
                    usize::try_from(metadata.len()).map_err(|_| {
                        format!("Frame photo {id} is larger than this host can count")
                    })?,
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "Frame photo {id} is missing from the simulator shelf"
                ));
            }
            Err(error) => return Err(format!("inspect Frame photo {id}: {error}")),
        }
    }
    Ok(sizes)
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
    use super::{
        base64_decode, capacity_bytes, parse_push, parse_remove, parse_target, transfer_script,
        Target, MANIFEST,
    };
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
            ("family", Target::Device("192.0.2.1".into()), Fit::Pad, true)
        );
    }

    #[test]
    fn parses_simulator_target() {
        assert_eq!(
            parse_push(&["harbour.png".into(), "--sim".into()]).expect("parse"),
            ("harbour.png", Target::Sim, Fit::Crop, false)
        );
        assert_eq!(
            parse_target(&["--sim".into()]).expect("target"),
            Target::Sim
        );
        assert_eq!(
            parse_remove(&["photo-abc".into(), "--sim".into()]).expect("remove"),
            ("photo-abc".into(), Target::Sim)
        );
        assert!(parse_push(&[
            "harbour.png".into(),
            "--sim".into(),
            "--device".into(),
            "192.0.2.1".into()
        ])
        .is_err());
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

    #[test]
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
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
