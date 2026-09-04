//! Assign Deck pads on the computer and push the layout the reader paints.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: kobo deck init [--home DIR]\n\
                     \x20      kobo deck set PAD [--page NAME] --label LABEL [--detail TEXT] \
                     (--run CMD | --url URL | --launch APP) [--confirm] [--home DIR]\n\
                     \x20      kobo deck ls [--home DIR]\n\
                     \x20      kobo deck show [--json] [--home DIR]\n\
                     \x20      kobo deck push (--sim | --device IP | --out PATH) [--home DIR]\n\
                     Assign a pad (1-12) on the computer-owned deck.toml. --launch and --url\n\
                     become shell commands Sidekick runs from the owner's home directory.";
const DEFAULT_PAGE: &str = "Home";
const MAX_PAGES: usize = 6;
const MAX_KEYS: usize = 12;
const MAX_LABEL: usize = 16;
const MAX_DETAIL: usize = 40;
const MAX_PAGE_NAME: usize = 24;
const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/state/deck";
const PAIRED_KEY: &str = "paired";
const CACHE_KEY: &str = "cache.deck-cache";
const LOCAL_PAIRING: &str = "local|assigned";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Key {
    label: String,
    detail: String,
    run: String,
    confirm: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Page {
    name: String,
    keys: Vec<Key>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct Layout {
    pages: Vec<Page>,
}

enum Destination<'a> {
    Device(&'a str),
    Simulator,
    File(&'a str),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("init") => init(&arguments[1..]),
        Some("set") => set(&arguments[1..]),
        Some("ls") => list(&arguments[1..], false),
        Some("show") => show(&arguments[1..]),
        Some("push") => push(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    let home = parse_home_only(arguments)?;
    let directory = config_dir(home)?;
    fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    println!("Deck config directory ready: {}", directory.display());
    println!("Assign pads with kobo deck set, then kobo deck push --sim or --device IP.");
    Ok(())
}

fn set(arguments: &[String]) -> Result<(), String> {
    let assignment = parse_set(arguments)?;
    let path = config_path(assignment.home)?;
    let mut layout = if path.exists() {
        load(&path)?
    } else {
        Layout::default()
    };
    let page_name = assignment.page.unwrap_or_else(|| {
        layout
            .pages
            .first()
            .map_or_else(|| DEFAULT_PAGE.to_owned(), |page| page.name.clone())
    });
    let pad = assignment.pad;
    {
        let page = page_mut(&mut layout, &page_name)?;
        if pad > page.keys.len() + 1 {
            return Err(format!(
                "pad {pad} is past the next free slot; assign pad {} first",
                page.keys.len() + 1
            ));
        }
        let key = Key {
            label: assignment.label,
            detail: assignment.detail,
            run: assignment.run,
            confirm: assignment.confirm,
        };
        if pad == page.keys.len() + 1 {
            page.keys.push(key);
        } else {
            page.keys[pad - 1] = key;
        }
    }
    write_layout(&path, &layout)?;
    let assigned = layout
        .page(&page_name)
        .and_then(|page| page.keys.get(pad - 1))
        .expect("just assigned");
    println!(
        "Assigned pad {pad} on '{page_name}': {} · {}",
        assigned.label, assigned.detail
    );
    Ok(())
}

fn list(arguments: &[String], json: bool) -> Result<(), String> {
    let home = parse_home_only(arguments)?;
    let path = config_path(home)?;
    if !path.exists() {
        return Err(format!(
            "no deck.toml at {}; run kobo deck set to assign pads",
            path.display()
        ));
    }
    let layout = load(&path)?;
    if json {
        println!("{}", layout.to_json());
        return Ok(());
    }
    if layout.pages.is_empty() {
        println!("No pages assigned.");
        return Ok(());
    }
    for page in &layout.pages {
        println!("{}", page.name);
        for (index, key) in page.keys.iter().enumerate() {
            let confirm = if key.confirm { "  confirm" } else { "" };
            println!(
                "  {:<2}  {:<16}  {:<40}  {}{confirm}",
                index + 1,
                key.label,
                key.detail,
                key.run
            );
        }
    }
    Ok(())
}

fn show(arguments: &[String]) -> Result<(), String> {
    let mut json = false;
    let mut home = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            "--home" => {
                home = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    list(&home_args(home), json)
}

fn push(arguments: &[String]) -> Result<(), String> {
    let (destination, home) = parse_push(arguments)?;
    let path = config_path(home)?;
    if !path.exists() {
        return Err(format!(
            "no deck.toml at {}; run kobo deck set first",
            path.display()
        ));
    }
    let layout = load(&path)?;
    if layout.pages.is_empty() || layout.pages.iter().any(|page| page.keys.is_empty()) {
        return Err("assign at least one pad before pushing a layout".to_owned());
    }
    let snapshot = layout.to_json();
    match destination {
        Destination::Simulator => {
            write_store(&sim_root(), LOCAL_PAIRING.as_bytes(), snapshot.as_bytes())?;
            println!(
                "Pushed {} pad(s) to the simulator store; Deck opens on the assigned grid.",
                layout.pad_count()
            );
            Ok(())
        }
        Destination::Device(host) => transfer(host, &snapshot, layout.pad_count()),
        Destination::File(output) => {
            write_atomic(Path::new(output), snapshot.as_bytes())?;
            println!(
                "Wrote {} pad(s) to {output} ({} bytes).",
                layout.pad_count(),
                snapshot.len()
            );
            Ok(())
        }
    }
}

struct Assignment<'a> {
    pad: usize,
    page: Option<String>,
    label: String,
    detail: String,
    run: String,
    confirm: bool,
    home: Option<&'a str>,
}

fn parse_set(arguments: &[String]) -> Result<Assignment<'_>, String> {
    let Some(pad) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    let pad = pad
        .parse::<usize>()
        .ok()
        .filter(|pad| (1..=MAX_KEYS).contains(pad))
        .ok_or_else(|| format!("pad must be 1 through {MAX_KEYS}"))?;
    let mut page = None;
    let mut label = None;
    let mut detail = None;
    let mut run = None;
    let mut url = None;
    let mut launch = None;
    let mut confirm = false;
    let mut home = None;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--page" => {
                page = Some(owned_flag(arguments, index, "--page")?);
                index += 2;
            }
            "--label" => {
                label = Some(owned_flag(arguments, index, "--label")?);
                index += 2;
            }
            "--detail" => {
                detail = Some(owned_flag(arguments, index, "--detail")?);
                index += 2;
            }
            "--run" => {
                run = Some(owned_flag(arguments, index, "--run")?);
                index += 2;
            }
            "--url" => {
                url = Some(owned_flag(arguments, index, "--url")?);
                index += 2;
            }
            "--launch" => {
                launch = Some(owned_flag(arguments, index, "--launch")?);
                index += 2;
            }
            "--confirm" => {
                confirm = true;
                index += 1;
            }
            "--home" => {
                home = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let kinds = [run.is_some(), url.is_some(), launch.is_some()]
        .into_iter()
        .filter(|set| *set)
        .count();
    if kinds != 1 {
        return Err(USAGE.to_owned());
    }
    let (default_label, default_detail, command) = if let Some(command) = run {
        ("Run".to_owned(), String::new(), command)
    } else if let Some(target) = url {
        if !target.starts_with("https://") && !target.starts_with("http://") {
            return Err("--url must start with http:// or https://".to_owned());
        }
        (
            url_label(&target),
            url_detail(&target),
            open_url_command(&target),
        )
    } else {
        let app = launch.expect("launch");
        (
            title_case(&app, MAX_LABEL),
            format!("launch {app}"),
            launch_command(&app),
        )
    };
    let label = label.unwrap_or(default_label);
    let detail = detail.unwrap_or(default_detail);
    validate_label(&label)?;
    validate_detail(&detail)?;
    if command.trim().is_empty() {
        return Err("the assigned command is empty".to_owned());
    }
    if let Some(name) = &page {
        validate_page_name(name)?;
    }
    Ok(Assignment {
        pad,
        page,
        label,
        detail,
        run: command,
        confirm,
        home,
    })
}

fn parse_push(arguments: &[String]) -> Result<(Destination<'_>, Option<&str>), String> {
    let mut host = None;
    let mut sim = false;
    let mut output = None;
    let mut home = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            flag if super::is_device_flag(flag) => {
                host = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            "--sim" => {
                sim = true;
                index += 1;
            }
            "--out" => {
                output = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            "--home" => {
                home = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let destination = match (host, sim, output) {
        (Some(host), false, None) => {
            if !super::valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            Destination::Device(host)
        }
        (None, true, None) => Destination::Simulator,
        (None, false, Some(path)) => Destination::File(path),
        _ => return Err(USAGE.to_owned()),
    };
    Ok((destination, home))
}

fn parse_home_only(arguments: &[String]) -> Result<Option<&str>, String> {
    match arguments {
        [] => Ok(None),
        [flag, home] if flag == "--home" => Ok(Some(home.as_str())),
        _ => Err(USAGE.to_owned()),
    }
}

fn home_args(home: Option<&str>) -> Vec<String> {
    match home {
        Some(home) => vec!["--home".into(), home.into()],
        None => Vec::new(),
    }
}

fn owned_flag(arguments: &[String], index: usize, flag: &str) -> Result<String, String> {
    arguments
        .get(index + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))
}

fn config_dir(home: Option<&str>) -> Result<PathBuf, String> {
    if let Some(home) = home {
        return Ok(PathBuf::from(home));
    }
    let Some(user) = env::var_os("HOME") else {
        return Err(format!("no HOME to look in; pass --home DIR\n{USAGE}"));
    };
    Ok(PathBuf::from(user).join(".config/kobo/sidekick"))
}

fn config_path(home: Option<&str>) -> Result<PathBuf, String> {
    Ok(config_dir(home)?.join("deck.toml"))
}

fn sim_root() -> PathBuf {
    env::temp_dir().join("cobalt-sim-state").join("deck")
}

fn page_mut<'a>(layout: &'a mut Layout, name: &str) -> Result<&'a mut Page, String> {
    if let Some(index) = layout.pages.iter().position(|page| page.name == name) {
        return Ok(&mut layout.pages[index]);
    }
    if layout.pages.len() >= MAX_PAGES {
        return Err(format!("a deck may have at most {MAX_PAGES} pages"));
    }
    layout.pages.push(Page {
        name: name.to_owned(),
        keys: Vec::new(),
    });
    Ok(layout.pages.last_mut().expect("just pushed"))
}

impl Layout {
    fn page(&self, name: &str) -> Option<&Page> {
        self.pages.iter().find(|page| page.name == name)
    }

    fn pad_count(&self) -> usize {
        self.pages.iter().map(|page| page.keys.len()).sum()
    }

    fn to_json(&self) -> String {
        let pages = self
            .pages
            .iter()
            .map(|page| {
                let keys = page
                    .keys
                    .iter()
                    .map(|key| {
                        kobo_json::ObjectBuilder::new()
                            .set("id", stable_id(&page.name, &key.label, &key.run).as_str())
                            .set("label", key.label.as_str())
                            .set("detail", key.detail.as_str())
                            .set("confirm", key.confirm)
                            .set("state", "idle")
                            .build()
                    })
                    .collect();
                kobo_json::ObjectBuilder::new()
                    .set("name", page.name.as_str())
                    .set("keys", kobo_json::Value::Array(keys))
                    .build()
            })
            .collect();
        kobo_json::ObjectBuilder::new()
            .set("version", "1")
            .set("pages", kobo_json::Value::Array(pages))
            .build()
            .to_json()
    }
}

fn stable_id(page: &str, label: &str, run: &str) -> String {
    let mut bytes = Vec::with_capacity(page.len() + label.len() + run.len() + 2);
    bytes.extend_from_slice(page.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(label.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(run.as_bytes());
    crate::sha256::hex_digest(&bytes)
}

fn launch_command(app: &str) -> String {
    let app = shell_quote(app);
    if cfg!(target_os = "macos") {
        format!("open -a {app}")
    } else {
        format!("gtk-launch {app} || {app}")
    }
}

fn open_url_command(url: &str) -> String {
    let url = shell_quote(url);
    if cfg!(target_os = "macos") {
        format!("open {url}")
    } else {
        format!("xdg-open {url}")
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn url_label(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);
    let host = host.trim_start_matches("www.");
    let name = host.split('.').next().unwrap_or(host);
    title_case(name, MAX_LABEL)
}

fn url_detail(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);
    truncate(host, MAX_DETAIL)
}

fn title_case(value: &str, max: usize) -> String {
    let mut title = String::new();
    let mut start = true;
    for character in value.chars() {
        if start {
            for upper in character.to_uppercase() {
                title.push(upper);
            }
            start = false;
        } else {
            title.push(character);
        }
        if title.chars().count() >= max {
            break;
        }
    }
    if title.is_empty() {
        "Pad".to_owned()
    } else {
        title
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn validate_page_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_PAGE_NAME {
        return Err(format!(
            "each page name must be 1 to {MAX_PAGE_NAME} characters"
        ));
    }
    Ok(())
}

fn validate_label(label: &str) -> Result<(), String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_LABEL {
        return Err(format!("a label must be 1 to {MAX_LABEL} characters"));
    }
    Ok(())
}

fn validate_detail(detail: &str) -> Result<(), String> {
    if detail.chars().count() > MAX_DETAIL {
        return Err(format!("detail must be at most {MAX_DETAIL} characters"));
    }
    Ok(())
}

fn load(path: &Path) -> Result<Layout, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    parse_toml(&source).map_err(|error| format!("{}: {error}", path.display()))
}

fn parse_toml(source: &str) -> Result<Layout, String> {
    let mut pages = Vec::new();
    let mut current: Option<Page> = None;
    let mut key: Option<Key> = None;
    let mut expecting_key = false;
    for (number, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[page]]" {
            finish_key(&mut current, key.take())?;
            if let Some(page) = current.take() {
                pages.push(page);
            }
            current = Some(Page {
                name: String::new(),
                keys: Vec::new(),
            });
            expecting_key = false;
            continue;
        }
        if line == "[[page.key]]" {
            finish_key(&mut current, key.take())?;
            if current.is_none() {
                return Err(format!("key on line {} has no page", number + 1));
            }
            key = Some(Key {
                label: String::new(),
                detail: String::new(),
                run: String::new(),
                confirm: false,
            });
            expecting_key = true;
            continue;
        }
        let Some((field, value)) = line.split_once('=') else {
            return Err(format!("line {} is not a field", number + 1));
        };
        let field = field.trim();
        let value = parse_toml_value(value.trim(), number + 1)?;
        if expecting_key {
            let Some(key) = key.as_mut() else {
                return Err(format!("line {} is not inside a key", number + 1));
            };
            match field {
                "label" => key.label = value,
                "detail" => key.detail = value,
                "run" => key.run = value,
                "confirm" => key.confirm = parse_bool(&value, number + 1)?,
                other => {
                    return Err(format!(
                        "unknown key field '{other}' on line {}",
                        number + 1
                    ))
                }
            }
        } else {
            let Some(page) = current.as_mut() else {
                return Err(format!("line {} is not inside a page", number + 1));
            };
            if field == "name" {
                page.name = value;
            } else {
                return Err(format!(
                    "unknown page field '{field}' on line {}",
                    number + 1
                ));
            }
        }
    }
    finish_key(&mut current, key.take())?;
    if let Some(page) = current.take() {
        pages.push(page);
    }
    if pages.is_empty() {
        return Err("deck.toml needs between 1 and 6 pages".to_owned());
    }
    for page in &pages {
        validate_page_name(&page.name)?;
        if page.keys.is_empty() {
            return Err(format!("page '{}' needs between 1 and 12 keys", page.name));
        }
        for key in &page.keys {
            validate_label(&key.label)?;
            validate_detail(&key.detail)?;
            if key.run.trim().is_empty() {
                return Err(format!(
                    "key '{}' on page '{}' has no command",
                    key.label, page.name
                ));
            }
        }
    }
    if pages.len() > MAX_PAGES {
        return Err("deck.toml needs between 1 and 6 pages".to_owned());
    }
    Ok(Layout { pages })
}

fn finish_key(page: &mut Option<Page>, key: Option<Key>) -> Result<(), String> {
    if let Some(key) = key {
        let Some(page) = page.as_mut() else {
            return Err("a key has no page".to_owned());
        };
        page.keys.push(key);
    }
    Ok(())
}

fn parse_toml_value(value: &str, line: usize) -> Result<String, String> {
    if value == "true" || value == "false" {
        return Ok(value.to_owned());
    }
    let Some(body) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Err(format!("line {line} needs a quoted string or true/false"));
    };
    let mut decoded = String::new();
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some('\\') => decoded.push('\\'),
                Some('"') => decoded.push('"'),
                Some('n') => decoded.push('\n'),
                Some(other) => {
                    decoded.push('\\');
                    decoded.push(other);
                }
                None => return Err(format!("line {line} has a dangling escape")),
            }
        } else {
            decoded.push(character);
        }
    }
    Ok(decoded)
}

fn parse_bool(value: &str, line: usize) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("line {line} confirm must be true or false")),
    }
}

fn write_layout(path: &Path, layout: &Layout) -> Result<(), String> {
    let mut body = String::new();
    for page in &layout.pages {
        body.push_str("[[page]]\n");
        body.push_str("name = ");
        push_toml_string(&mut body, &page.name);
        body.push('\n');
        for key in &page.keys {
            body.push_str("\n[[page.key]]\n");
            body.push_str("label = ");
            push_toml_string(&mut body, &key.label);
            body.push_str("\ndetail = ");
            push_toml_string(&mut body, &key.detail);
            body.push_str("\nrun = ");
            push_toml_string(&mut body, &key.run);
            body.push_str("\nconfirm = ");
            body.push_str(if key.confirm { "true" } else { "false" });
            body.push('\n');
        }
        body.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    write_atomic(path, body.as_bytes())
}

fn push_toml_string(body: &mut String, value: &str) {
    body.push('"');
    for character in value.chars() {
        match character {
            '\\' => body.push_str("\\\\"),
            '"' => body.push_str("\\\""),
            '\n' => body.push_str("\\n"),
            other => body.push(other),
        }
    }
    body.push('"');
}

fn write_store(root: &Path, pairing: &[u8], snapshot: &[u8]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("create {}: {error}", root.display()))?;
    write_atomic(&root.join(PAIRED_KEY), pairing)?;
    write_atomic(&root.join(CACHE_KEY), snapshot)?;
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} is not a usable path", path.display()))?;
    let temporary = path.with_file_name(format!(".{name}.writing"));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("flush {}: {error}", temporary.display()))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ignored = fs::remove_file(&temporary);
        format!("publish {}: {error}", path.display())
    })
}

fn transfer(host: &str, snapshot: &str, pads: usize) -> Result<(), String> {
    let pairing = super::base64_encode(LOCAL_PAIRING.as_bytes());
    let encoded = super::base64_encode(snapshot.as_bytes());
    let script = format!(
        "set -eu\n\
         root='{DEVICE_ROOT}'\n\
         mkdir -p \"$root\"\n\
         chmod 700 \"$root\"\n\
         write() {{\n\
           key=\"$1\"\n\
           partial=\"$root/.$key.writing\"\n\
           base64 -d > \"$partial\"\n\
           chmod 600 \"$partial\"\n\
           mv -f \"$partial\" \"$root/$key\"\n\
         }}\n\
         write '{PAIRED_KEY}' <<'KOBO_DECK_PAIR'\n\
         {pairing}\n\
         KOBO_DECK_PAIR\n\
         write '{CACHE_KEY}' <<'KOBO_DECK_LAYOUT'\n\
         {encoded}\n\
         KOBO_DECK_LAYOUT\n\
         sync\n\
         printf 'Pushed {pads} Deck pad(s)\\n'\n"
    );
    let output = remote(host, &script)?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn remote(host: &str, script: &str) -> Result<super::RemoteShellOutput, String> {
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        script,
        super::REMOTE_COMMAND_TIMEOUT,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the Deck transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        command, config_path, launch_command, load, open_url_command, parse_toml, sim_root,
        write_store, CACHE_KEY, LOCAL_PAIRING, PAIRED_KEY,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn home() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root =
            std::env::temp_dir().join(format!("cobalt-deck-cli-{}-{unique}", std::process::id()));
        fs::create_dir_all(&root).expect("home");
        root
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn help_succeeds() {
        command(&["--help".into()]).expect("deck help");
    }

    #[test]
    fn assignment_round_trips_into_the_json_layout_the_app_decodes() {
        let root = home();
        let home = root.display().to_string();
        command(&args(&["set", "1", "--launch", "todo", "--home", &home])).expect("pad 1");
        command(&args(&[
            "set",
            "2",
            "--url",
            "https://example.com",
            "--home",
            &home,
        ]))
        .expect("pad 2");
        let layout = load(&config_path(Some(&home)).expect("path")).expect("load");
        assert_eq!(layout.pages.len(), 1);
        assert_eq!(layout.pages[0].name, "Home");
        assert_eq!(layout.pages[0].keys[0].label, "Todo");
        assert_eq!(layout.pages[0].keys[0].detail, "launch todo");
        assert_eq!(layout.pages[0].keys[0].run, launch_command("todo"));
        assert_eq!(layout.pages[0].keys[1].label, "Example");
        assert_eq!(layout.pages[0].keys[1].detail, "example.com");
        assert_eq!(
            layout.pages[0].keys[1].run,
            open_url_command("https://example.com")
        );
        let json = layout.to_json();
        assert!(json.contains("\"label\":\"Todo\""), "{json}");
        assert!(json.contains("\"detail\":\"launch todo\""), "{json}");
        assert!(json.contains("\"label\":\"Example\""), "{json}");
        assert!(json.contains("\"detail\":\"example.com\""), "{json}");
        let decoded = kobo_json::parse(&json).expect("json");
        let pages = decoded.get("pages").and_then(|value| value.as_array());
        assert_eq!(pages.map(<[_]>::len), Some(1));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn push_sim_seeds_the_store_the_app_opens_from() {
        let root = home();
        let home = root.display().to_string();
        command(&args(&[
            "set", "1", "--run", "true", "--label", "Test", "--home", &home,
        ]))
        .expect("set");
        let store = root.join("sim-store");
        let layout = load(&config_path(Some(&home)).expect("path")).expect("load");
        write_store(
            &store,
            LOCAL_PAIRING.as_bytes(),
            layout.to_json().as_bytes(),
        )
        .expect("store");
        assert_eq!(
            fs::read_to_string(store.join(PAIRED_KEY)).expect("paired"),
            LOCAL_PAIRING
        );
        let cached = fs::read_to_string(store.join(CACHE_KEY)).expect("cache");
        assert!(cached.contains("\"label\":\"Test\""), "{cached}");
        assert_eq!(
            sim_root().file_name().and_then(|name| name.to_str()),
            Some("deck")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn parser_accepts_the_file_set_writes() {
        let source = "[[page]]\nname = \"Build\"\n\n[[page.key]]\nlabel = \"Test\"\ndetail = \"cargo test\"\nrun = \"cargo test\"\nconfirm = true\n";
        let layout = parse_toml(source).expect("parse");
        assert!(layout.pages[0].keys[0].confirm);
        assert_eq!(layout.pages[0].keys[0].run, "cargo test");
    }
}
