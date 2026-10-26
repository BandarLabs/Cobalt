//! Build a selected set of local SDK apps and run their real launcher journeys.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn run(arguments: &[String]) -> Result<(), String> {
    let (address, requested) = arguments_for_runtime(arguments)?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version=1"])
        .current_dir(&workspace)
        .output()
        .map_err(|error| format!("read workspace: {error}"))?;
    if !output.status.success() {
        return Err("could not read the local Cobalt workspace".into());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let selected = select_programs(&metadata, requested.as_ref())?;
    println!("Building {} apps for runtime simulation…", selected.len());
    let mut build = Command::new("cargo");
    build
        .args(["build", "--message-format=json"])
        .current_dir(&workspace);
    for (package, _) in selected.values() {
        build.args(["-p", package]);
    }
    let output = build
        .output()
        .map_err(|error| format!("build simulator apps: {error}"))?;
    if !output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return Err("simulator app build failed".into());
    }
    let executables = super::build_executables(&String::from_utf8_lossy(&output.stdout));
    let mut programs = BTreeMap::new();
    for (id, (_, binary)) in selected {
        let executable: PathBuf = executables
            .iter()
            .find(|path| path.file_name().is_some_and(|name| name == binary.as_str()))
            .ok_or_else(|| format!("build did not produce {binary}"))?
            .clone();
        programs.insert(
            id,
            kobo_sim::runtime::Program {
                source: super::dev_capture_source(&executable)?,
                executable,
            },
        );
    }
    let session = super::DevSessionGuard::new()?;
    let server = kobo_sim::AppServer::bind(address, &session.socket)
        .map_err(|error| error.to_string())?
        .with_runtime_navigation();
    kobo_sim::runtime::run(server, &session.socket, &programs).map_err(|error| error.to_string())
}

fn arguments_for_runtime(arguments: &[String]) -> Result<(&str, Option<BTreeSet<String>>), String> {
    let mut address = None;
    let mut requested = None;
    let mut args = arguments.iter();
    while let Some(arg) = args.next() {
        if arg == "--apps" {
            if requested.is_some() {
                return Err("--apps may be used once".into());
            }
            requested = Some(
                args.next()
                    .ok_or("--apps needs comma-separated app identities")?
                    .split(',')
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>(),
            );
        } else if !arg.starts_with('-') && address.is_none() {
            address = Some(arg.as_str());
        } else {
            return Err("usage: kobo dev --runtime [address] [--apps todo,store]".into());
        }
    }
    Ok((address.unwrap_or("127.0.0.1:8787"), requested))
}

fn select_programs(
    metadata: &serde_json::Value,
    requested: Option<&BTreeSet<String>>,
) -> Result<BTreeMap<String, (String, String)>, String> {
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("workspace has no packages")?;
    let mut selected = BTreeMap::new();
    for package in packages {
        let package_name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("package has no name")?;
        let Some(manifest) = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let directory = Path::new(manifest)
            .parent()
            .ok_or("package has no directory")?;
        let contribution = directory.join("cobalt-app.json");
        let id = if contribution.is_file() {
            let source =
                std::fs::read_to_string(&contribution).map_err(|error| error.to_string())?;
            kobo_catalog::App::parse(&source)?.id
        } else if matches!(
            package_name,
            "kobo-launcher" | "kobo-store" | "kobo-settings" | "kobo-books" | "kobo-terminal"
        ) {
            package_name.trim_start_matches("kobo-").to_owned()
        } else {
            continue;
        };
        if id != "launcher" && requested.as_ref().is_some_and(|names| !names.contains(&id)) {
            continue;
        }
        let targets = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .ok_or("package has no targets")?;
        let binaries = targets
            .iter()
            .filter(|target| {
                target
                    .get("kind")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            })
            .collect::<Vec<_>>();
        if binaries.len() != 1 {
            return Err(format!("{id} must have exactly one app binary"));
        }
        let name = binaries[0]
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("binary has no name")?;
        if selected
            .insert(id.clone(), (package_name.to_owned(), name.to_owned()))
            .is_some()
        {
            return Err(format!("duplicate app identity {id}"));
        }
    }
    if let Some(requested) = requested {
        if let Some(missing) = requested.iter().find(|name| !selected.contains_key(*name)) {
            return Err(format!("{missing} is not a registered local app"));
        }
    }
    if !selected.contains_key("launcher") {
        return Err("this workspace has no launcher".into());
    }
    Ok(selected)
}
