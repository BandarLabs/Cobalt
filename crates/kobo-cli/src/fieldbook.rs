//! Validate and transfer Fieldbook packs, and receive prepared eBird checklists.
use kobo_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = "packs.v1";
const MAX_MANIFEST: usize = 512 * 1024;
const MAX_CHECKLIST: usize = 4 * 1024 * 1024;
const DEVICE_DATA: &str = "/mnt/onboard/.adds/cobalt/data/fieldbook";
const DEVICE_STATE: &str = "/mnt/onboard/.adds/cobalt/state/fieldbook";
const CHECKLIST: &str = "export/checklist.csv";
const USAGE: &str = "usage: kobo fieldbook inspect PACK.json\n\
                     \x20      kobo fieldbook push PACK.json (--sim | --device IP)\n\
                     \x20      kobo fieldbook ls (--sim | --device IP)\n\
                     \x20      kobo fieldbook export (--sim | --device IP) --out FILE.csv\n\
                     Pack manifests are limited to 512 KiB. Export receives the checklist prepared in Fieldbook.";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Summary {
    packs: usize,
    species: usize,
    failures: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Sim,
    Device(String),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("inspect") if arguments.len() == 2 => {
            let bytes = bounded(Path::new(&arguments[1]), MAX_MANIFEST, "pack manifest")?;
            print_summary(&inspect(&bytes)?);
            Ok(())
        }
        Some("push") if arguments.len() >= 3 => {
            let bytes = bounded(Path::new(&arguments[1]), MAX_MANIFEST, "pack manifest")?;
            let summary = inspect(&bytes)?;
            publish(parse_target(&arguments[2..])?, &bytes)?;
            print!("Fieldbook pack ready: ");
            print_summary(&summary);
            Ok(())
        }
        Some("ls") => {
            let target = parse_target(&arguments[1..])?;
            let bytes = read_target(&target, MANIFEST, MAX_MANIFEST, true)?;
            print_summary(&inspect(&bytes)?);
            Ok(())
        }
        Some("export") => export(&arguments[1..]),
        _ => Err(USAGE.into()),
    }
}

fn parse_target(arguments: &[String]) -> Result<Target, String> {
    match arguments {
        [flag] if flag == "--sim" => Ok(Target::Sim),
        [flag, host] if super::is_device_flag(flag) && super::valid_device_host(host) => {
            Ok(Target::Device(host.clone()))
        }
        _ => Err(USAGE.into()),
    }
}
fn bounded(path: &Path, max: usize, label: &str) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > max as u64 {
        return Err(format!("the {label} exceeds the {} KiB limit", max / 1024));
    }
    fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))
}
fn text<'a>(value: &'a Value, key: &str, what: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{what} needs {key}"))
}
fn inspect(bytes: &[u8]) -> Result<Summary, String> {
    if bytes.len() > MAX_MANIFEST {
        return Err("the pack manifest exceeds the 512 KiB limit".into());
    }
    let root =
        kobo_json::parse(std::str::from_utf8(bytes).map_err(|_| "the pack manifest is not UTF-8")?)
            .map_err(|e| format!("invalid Fieldbook pack: {e}"))?;
    if text(&root, "format", "manifest")? != "fieldbook-shelf"
        || text(&root, "version", "manifest")? != "1"
    {
        return Err("this is not a Fieldbook shelf version 1 manifest".into());
    }
    let packs = root
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("the Fieldbook manifest needs packs")?;
    let failures = root
        .get("failures")
        .and_then(Value::as_array)
        .ok_or("the Fieldbook manifest needs failures")?;
    let mut species = 0;
    for pack in packs {
        for key in ["id", "title", "region", "issued"] {
            let _ = text(pack, key, "a Fieldbook pack")?;
        }
        let entries = pack
            .get("species")
            .and_then(Value::as_array)
            .ok_or("a Fieldbook pack needs species")?;
        for bird in entries {
            for key in ["code", "common", "scientific"] {
                let _ = text(bird, key, "a Fieldbook species")?;
            }
        }
        species += entries.len();
    }
    for failure in failures {
        let _ = text(failure, "input", "a Fieldbook failure")?;
        let _ = text(failure, "reason", "a Fieldbook failure")?;
    }
    Ok(Summary {
        packs: packs.len(),
        species,
        failures: failures.len(),
    })
}
fn print_summary(s: &Summary) {
    println!(
        "{} pack(s) · {} species · {} import failure(s)",
        s.packs, s.species, s.failures
    );
}
fn sim_data() -> PathBuf {
    kobo_sim::simulated_data_root("fieldbook")
}
fn publish(target: Target, bytes: &[u8]) -> Result<(), String> {
    match target {
        Target::Sim => atomic(&sim_data().join(MANIFEST), bytes),
        Target::Device(host) => remote_write(&host, DEVICE_DATA, MANIFEST, bytes),
    }
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("destination has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let partial = parent.join(format!(".{MANIFEST}.{}.writing", std::process::id()));
    let result = (|| {
        fs::write(&partial, bytes).map_err(|e| e.to_string())?;
        fs::rename(&partial, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}
fn remote_write(host: &str, root: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let encoded = super::base64_encode(bytes);
    let count = bytes.len();
    let digest = kobo_net::sha256::hex_digest(bytes);
    let script=format!("set -eu\nroot='{root}'\nmkdir -p \"$root\"\npartial=\"$root/.{name}.$$.writing\"\ntrap 'rm -f \"$partial\"' EXIT HUP INT TERM\nbase64 -d > \"$partial\" <<'FIELD_BOOK'\n{encoded}\nFIELD_BOOK\ntest \"$(wc -c < \"$partial\")\" = '{count}'\nset -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$root/{name}\"\nsync\n");
    let out = super::run_remote_shell(
        &format!("root@{host}"),
        &script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if out.status.success() {
        Ok(())
    } else {
        Err("the reader refused the Fieldbook transfer".into())
    }
}
fn read_target(target: &Target, name: &str, max: usize, data: bool) -> Result<Vec<u8>, String> {
    match target {
        Target::Sim => {
            let root = if data {
                sim_data()
            } else {
                std::env::temp_dir().join("cobalt-sim-state/fieldbook")
            };
            bounded(&root.join(name), max, name)
        }
        Target::Device(host) => {
            let root = if data { DEVICE_DATA } else { DEVICE_STATE };
            let script=format!("set -eu\ntest -f '{root}/{name}' && test ! -L '{root}/{name}'\nhead -c {} '{root}/{name}'\n",max+1);
            let out = super::run_remote_shell(
                &format!("root@{host}"),
                &script,
                super::REMOTE_COMMAND_TIMEOUT,
            )
            .map_err(super::unreachable_device)?;
            if !out.status.success() || out.stdout.len() > max {
                return Err(format!("no valid {name} is available"));
            }
            Ok(out.stdout)
        }
    }
}
fn export(args: &[String]) -> Result<(), String> {
    let out_at = args.iter().position(|v| v == "--out").ok_or(USAGE)?;
    let output = args.get(out_at + 1).ok_or(USAGE)?;
    let mut target_args = args.to_vec();
    target_args.drain(out_at..=out_at + 1);
    let target = parse_target(&target_args)?;
    let bytes = read_target(&target, CHECKLIST, MAX_CHECKLIST, false)?;
    if bytes.is_empty() || !bytes.starts_with(b",,") {
        return Err("the prepared checklist is not eBird Checklist Format CSV".into());
    }
    let output = Path::new(output);
    if output.exists() {
        return Err(format!(
            "{} already exists; choose a new filename",
            output.display()
        ));
    }
    atomic_output(output, &bytes)?;
    println!(
        "Saved {}\nThe original remains in Fieldbook.",
        output.display()
    );
    Ok(())
}
fn atomic_output(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let part = parent.join(format!(".fieldbook-export-{}.part", std::process::id()));
    fs::write(&part, bytes).map_err(|e| e.to_string())?;
    let r = fs::hard_link(&part, path).map_err(|e| format!("save checklist: {e}"));
    let _ = fs::remove_file(part);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Vec<u8> {
        br#"{"format":"fieldbook-shelf","version":"1","packs":[{"id":"cp","title":"Central Park","region":"US-NY","issued":"2026-09-18","species":[{"code":"AMRO","common":"American Robin","scientific":"Turdus migratorius"}]}],"failures":[]}"#.to_vec()
    }
    #[test]
    fn validates_contract() {
        assert_eq!(
            inspect(&sample()).unwrap(),
            Summary {
                packs: 1,
                species: 1,
                failures: 0
            }
        );
    }
    #[test]
    fn rejects_wrong_schema_and_incomplete_species() {
        assert!(inspect(b"{}").is_err());
        assert!(inspect(
            br#"{"format":"fieldbook-shelf","version":"1","packs":[],"failures":null}"#
        )
        .is_err());
    }
}
