//! `kobo clip install` and `uninstall`: Send to Kobo in a browser's
//! right-click menu.
//!
//! A browser extension cannot run `kobo` by itself. It can only talk to a
//! program the browser was told about, through a small manifest in a place
//! each browser looks, which names the program and the one extension
//! allowed to start it. So install writes that manifest for each browser,
//! a launcher that runs `kobo clip native-host`, and a copy of the extension
//! the owner loads once. Safari takes extensions only inside a signed Mac
//! app, so it gets a Quick Action in the Services menu instead.
//!
//! Where each browser looks and what goes in the manifest are plain
//! functions of the browser and the platform, so they are tested on every
//! platform rather than only the one the tests run on.

use super::clip_delivery as delivery;
use std::fs;
use std::path::{Path, PathBuf};

/// The name browsers know the host by.
pub const HOST_NAME: &str = "io.github.bandarlabs.cobalt_clip";

/// The extension's Chromium ID, fixed by the public key in its manifest.
pub const CHROMIUM_ID: &str = "oddgeegniblkiahbnpjbgjoafeanokgi";

/// The extension's Firefox ID, from its manifest.
pub const FIREFOX_ID: &str = "send-to-kobo@cobalt.bandarlabs.github.io";

/// The extension as it is copied out, file by file. Built into this binary,
/// so an installed `kobo` needs no source tree beside it.
const EXTENSION: &[(&str, &[u8])] = &[
    (
        "manifest.json",
        include_bytes!("../../../extensions/clip/manifest.json"),
    ),
    (
        "background.js",
        include_bytes!("../../../extensions/clip/background.js"),
    ),
    (
        "icons/icon-16.png",
        include_bytes!("../../../extensions/clip/icons/icon-16.png"),
    ),
    (
        "icons/icon-32.png",
        include_bytes!("../../../extensions/clip/icons/icon-32.png"),
    ),
    (
        "icons/icon-48.png",
        include_bytes!("../../../extensions/clip/icons/icon-48.png"),
    ),
    (
        "icons/icon-128.png",
        include_bytes!("../../../extensions/clip/icons/icon-128.png"),
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
    Chromium,
    Firefox,
    Safari,
}

impl Browser {
    pub const ALL: [Self; 6] = [
        Self::Chrome,
        Self::Edge,
        Self::Brave,
        Self::Chromium,
        Self::Firefox,
        Self::Safari,
    ];

    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|browser| browser.id() == name.to_ascii_lowercase())
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::Brave => "brave",
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Brave => "Brave",
            Self::Chromium => "Chromium",
            Self::Firefox => "Firefox",
            Self::Safari => "Safari",
        }
    }

    const fn is_firefox(self) -> bool {
        matches!(self, Self::Firefox)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Os {
    Mac,
    Linux,
    Windows,
}

impl Os {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Where a browser learns of the host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Registration {
    /// A manifest file in a folder the browser reads, on macOS and Linux.
    File(PathBuf),
    /// On Windows, a key under `HKEY_CURRENT_USER` whose default value names
    /// a manifest file kept with the companion's own files.
    Registry { key: String, manifest: PathBuf },
}

/// Where `browser` reads the host manifest on `os`, for the person whose
/// home is `home`. `config` is the companion's own folder. Per-user places
/// only, so nothing needs an administrator. `None` for Safari, which has no
/// native messaging for an extension outside a Mac app.
pub fn registration(browser: Browser, os: Os, home: &Path, config: &Path) -> Option<Registration> {
    let file = format!("{HOST_NAME}.json");
    let folder = match (os, browser) {
        (_, Browser::Safari) => return None,
        (Os::Windows, _) => {
            let vendor = match browser {
                Browser::Chrome => r"Google\Chrome",
                Browser::Edge => r"Microsoft\Edge",
                Browser::Brave => r"BraveSoftware\Brave-Browser",
                Browser::Chromium => "Chromium",
                Browser::Firefox => "Mozilla",
                Browser::Safari => return None,
            };
            let family = if browser.is_firefox() {
                "firefox"
            } else {
                "chromium"
            };
            return Some(Registration::Registry {
                key: format!(r"HKCU\Software\{vendor}\NativeMessagingHosts\{HOST_NAME}"),
                manifest: config.join("clip").join(family).join(file),
            });
        }
        (Os::Mac, Browser::Firefox) => home.join("Library/Application Support/Mozilla"),
        (Os::Mac, _) => home
            .join("Library/Application Support")
            .join(match browser {
                Browser::Chrome => "Google/Chrome",
                Browser::Edge => "Microsoft Edge",
                Browser::Brave => "BraveSoftware/Brave-Browser",
                _ => "Chromium",
            }),
        (Os::Linux, Browser::Firefox) => home.join(".mozilla"),
        (Os::Linux, _) => home.join(".config").join(match browser {
            Browser::Chrome => "google-chrome",
            Browser::Edge => "microsoft-edge",
            Browser::Brave => "BraveSoftware/Brave-Browser",
            _ => "chromium",
        }),
    };
    let hosts = match (os, browser) {
        (Os::Linux, Browser::Firefox) => "native-messaging-hosts",
        _ => "NativeMessagingHosts",
    };
    Some(Registration::File(folder.join(hosts).join(file)))
}

/// The folder a browser keeps its profiles in, whose presence says it is
/// installed and has been opened. `local` and `roaming` are Windows'
/// `LOCALAPPDATA` and `APPDATA`.
pub fn profile_folder(
    browser: Browser,
    os: Os,
    home: &Path,
    local: &Path,
    roaming: &Path,
) -> PathBuf {
    match os {
        Os::Mac => match browser {
            Browser::Safari => PathBuf::from("/Applications/Safari.app"),
            Browser::Firefox => home.join("Library/Application Support/Firefox"),
            _ => home
                .join("Library/Application Support")
                .join(match browser {
                    Browser::Chrome => "Google/Chrome",
                    Browser::Edge => "Microsoft Edge",
                    Browser::Brave => "BraveSoftware/Brave-Browser",
                    _ => "Chromium",
                }),
        },
        Os::Linux => match browser {
            Browser::Firefox => home.join(".mozilla/firefox"),
            Browser::Safari => PathBuf::new(),
            _ => home.join(".config").join(match browser {
                Browser::Chrome => "google-chrome",
                Browser::Edge => "microsoft-edge",
                Browser::Brave => "BraveSoftware/Brave-Browser",
                _ => "chromium",
            }),
        },
        Os::Windows => match browser {
            Browser::Firefox => roaming.join(r"Mozilla\Firefox"),
            Browser::Safari => PathBuf::new(),
            _ => local.join(match browser {
                Browser::Chrome => r"Google\Chrome\User Data",
                Browser::Edge => r"Microsoft\Edge\User Data",
                Browser::Brave => r"BraveSoftware\Brave-Browser\User Data",
                _ => r"Chromium\User Data",
            }),
        },
    }
}

/// Every place `browser` may read the host from, its own first.
///
/// Brave on a Mac was seen reading only Chrome's folder, though Brave's own
/// is where its documentation points, so Brave is registered in both. Its
/// uninstall takes away only its own, because Chrome's belongs to Chrome.
pub fn registrations(browser: Browser, os: Os, home: &Path, config: &Path) -> Vec<Registration> {
    let mut places: Vec<Registration> = registration(browser, os, home, config)
        .into_iter()
        .collect();
    if browser == Browser::Brave {
        places.extend(registration(Browser::Chrome, os, home, config));
    }
    places
}

/// The host manifest for `browser`, starting `launcher`, and only for this
/// extension: Chromium browsers name it by origin, Firefox by its ID.
pub fn host_manifest(browser: Browser, launcher: &Path) -> String {
    let allowed = if browser.is_firefox() {
        (
            "allowed_extensions",
            kobo_json::Value::Array(vec![kobo_json::Value::String(FIREFOX_ID.to_owned())]),
        )
    } else {
        (
            "allowed_origins",
            kobo_json::Value::Array(vec![kobo_json::Value::String(format!(
                "chrome-extension://{CHROMIUM_ID}/"
            ))]),
        )
    };
    kobo_json::ObjectBuilder::new()
        .set("name", HOST_NAME)
        .set("description", "Send to Kobo, through Cobalt's kobo command")
        .set("path", launcher.to_string_lossy().as_ref())
        .set("type", "stdio")
        .set(allowed.0, allowed.1)
        .build()
        .to_json_pretty()
}

/// The launcher's file name and contents. A manifest names a program and
/// no arguments, so this is the program, and it runs `kobo clip
/// native-host` with whatever the browser passes.
pub fn launcher(os: Os, kobo: &Path) -> (&'static str, String) {
    let kobo = kobo.to_string_lossy();
    match os {
        Os::Windows => (
            "kobo-clip-host.bat",
            format!("@echo off\r\n\"{kobo}\" clip native-host %*\r\n"),
        ),
        Os::Mac | Os::Linux => (
            "kobo-clip-host",
            format!(
                "#!/bin/sh\n# Started by the browser for the Send to Kobo extension.\nexec {} clip native-host \"$@\"\n",
                shell_quote(&kobo)
            ),
        ),
    }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `reg` arguments that point `key` at `manifest`.
pub fn registry_add(key: &str, manifest: &Path) -> Vec<String> {
    vec![
        "add".to_owned(),
        key.to_owned(),
        "/ve".to_owned(),
        "/t".to_owned(),
        "REG_SZ".to_owned(),
        "/d".to_owned(),
        manifest.to_string_lossy().into_owned(),
        "/f".to_owned(),
    ]
}

/// `reg` arguments that take `key` away.
pub fn registry_delete(key: &str) -> Vec<String> {
    vec!["delete".to_owned(), key.to_owned(), "/f".to_owned()]
}

/// What the owner does in the browser, in a line or two.
pub fn steps(browser: Browser, extension: &Path) -> String {
    let folder = extension.display();
    match browser {
        Browser::Chrome | Browser::Edge | Browser::Brave | Browser::Chromium => {
            let page = match browser {
                Browser::Edge => "edge://extensions",
                Browser::Brave => "brave://extensions",
                _ => "chrome://extensions",
            };
            format!(
                "{}: open {page}, turn on Developer mode, choose Load unpacked and pick\n  {folder}",
                browser.title()
            )
        }
        Browser::Firefox => format!(
            "Firefox: open about:debugging#/runtime/this-firefox, choose Load Temporary Add-on and pick\n  {}\n  Firefox forgets it when it restarts, so load it again after a restart.",
            extension.join("manifest.json").display()
        ),
        Browser::Safari => "Safari: right-click a link and choose Services, then Send to Kobo. For the page you are on, select its address and do the same.\n  A full Safari extension needs a signed Mac app, which this is not.".to_owned(),
    }
}

/// The Quick Action's folder in `~/Library/Services`.
pub fn safari_service(home: &Path) -> PathBuf {
    home.join("Library/Services/Send to Kobo.workflow")
}

/// The shell the Quick Action runs for each address Safari hands it.
pub fn safari_command(kobo: &Path) -> String {
    format!(
        "for page in \"$@\"; do\n  case \"$page\" in\n    http://*|https://*) {} clip \"$page\" --notify ;;\n  esac\ndone\n",
        shell_quote(&kobo.to_string_lossy())
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The Quick Action's two files: its `Info.plist`, which puts it in the
/// Services menu for addresses and text, and its workflow, one Run Shell
/// Script step.
pub fn safari_files(kobo: &Path) -> [(&'static str, String); 2] {
    let info = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>NSServices</key>\n\t<array>\n\t\t<dict>\n\
\t\t\t<key>NSMenuItem</key>\n\t\t\t<dict>\n\t\t\t\t<key>default</key>\n\t\t\t\t<string>Send to Kobo</string>\n\t\t\t</dict>\n\
\t\t\t<key>NSMessage</key>\n\t\t\t<string>runWorkflowAsService</string>\n\
\t\t\t<key>NSSendTypes</key>\n\t\t\t<array>\n\t\t\t\t<string>public.url</string>\n\t\t\t\t<string>public.utf8-plain-text</string>\n\t\t\t</array>\n\
\t\t</dict>\n\t</array>\n</dict>\n</plist>\n"
        .to_owned();
    let argument = |index: u8, name: &str, default: &str| {
        format!(
            "\t\t\t\t\t<key>{index}</key>\n\t\t\t\t\t<dict>\n\
             \t\t\t\t\t\t<key>default value</key>\n\t\t\t\t\t\t{default}\n\
             \t\t\t\t\t\t<key>name</key>\n\t\t\t\t\t\t<string>{name}</string>\n\
             \t\t\t\t\t\t<key>required</key>\n\t\t\t\t\t\t<string>0</string>\n\
             \t\t\t\t\t\t<key>type</key>\n\t\t\t\t\t\t<string>0</string>\n\
             \t\t\t\t\t\t<key>uuid</key>\n\t\t\t\t\t\t<string>{index}</string>\n\
             \t\t\t\t\t</dict>\n"
        )
    };
    let arguments = [
        argument(0, "inputMethod", "<integer>0</integer>"),
        argument(1, "source", "<string></string>"),
        argument(2, "COMMAND_STRING", "<string></string>"),
        argument(3, "CheckedForUserDefaultShell", "<false/>"),
        argument(4, "shell", "<string></string>"),
    ]
    .concat();
    let workflow = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>AMApplicationBuild</key>\n\t<string>523</string>\n\
\t<key>AMApplicationVersion</key>\n\t<string>2.10</string>\n\
\t<key>AMDocumentVersion</key>\n\t<string>2</string>\n\
\t<key>actions</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>action</key>\n\t\t\t<dict>\n\
\t\t\t\t<key>AMAccepts</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>Container</key>\n\t\t\t\t\t<string>List</string>\n\t\t\t\t\t<key>Optional</key>\n\t\t\t\t\t<true/>\n\t\t\t\t\t<key>Types</key>\n\t\t\t\t\t<array>\n\t\t\t\t\t\t<string>com.apple.cocoa.string</string>\n\t\t\t\t\t</array>\n\t\t\t\t</dict>\n\
\t\t\t\t<key>AMActionVersion</key>\n\t\t\t\t<string>2.0.3</string>\n\
\t\t\t\t<key>AMApplication</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>Automator</string>\n\t\t\t\t</array>\n\
\t\t\t\t<key>AMParameterProperties</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>COMMAND_STRING</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>CheckedForUserDefaultShell</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>inputMethod</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>shell</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t\t<key>source</key>\n\t\t\t\t\t<dict/>\n\t\t\t\t</dict>\n\
\t\t\t\t<key>AMProvides</key>\n\t\t\t\t<dict>\n\t\t\t\t\t<key>Container</key>\n\t\t\t\t\t<string>List</string>\n\t\t\t\t\t<key>Types</key>\n\t\t\t\t\t<array>\n\t\t\t\t\t\t<string>com.apple.cocoa.string</string>\n\t\t\t\t\t</array>\n\t\t\t\t</dict>\n\
\t\t\t\t<key>ActionBundlePath</key>\n\t\t\t\t<string>/System/Library/Automator/Run Shell Script.action</string>\n\
\t\t\t\t<key>ActionName</key>\n\t\t\t\t<string>Run Shell Script</string>\n\
\t\t\t\t<key>ActionParameters</key>\n\t\t\t\t<dict>\n\
\t\t\t\t\t<key>COMMAND_STRING</key>\n\t\t\t\t\t<string>{command}</string>\n\
\t\t\t\t\t<key>CheckedForUserDefaultShell</key>\n\t\t\t\t\t<true/>\n\
\t\t\t\t\t<key>inputMethod</key>\n\t\t\t\t\t<integer>1</integer>\n\
\t\t\t\t\t<key>shell</key>\n\t\t\t\t\t<string>/bin/sh</string>\n\
\t\t\t\t\t<key>source</key>\n\t\t\t\t\t<string></string>\n\
\t\t\t\t</dict>\n\
\t\t\t\t<key>BundleIdentifier</key>\n\t\t\t\t<string>com.apple.RunShellScript</string>\n\
\t\t\t\t<key>CFBundleVersion</key>\n\t\t\t\t<string>2.0.3</string>\n\
\t\t\t\t<key>CanShowSelectedItemsWhenRun</key>\n\t\t\t\t<false/>\n\
\t\t\t\t<key>CanShowWhenRun</key>\n\t\t\t\t<true/>\n\
\t\t\t\t<key>Category</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>AMCategoryUtilities</string>\n\t\t\t\t</array>\n\
\t\t\t\t<key>Class Name</key>\n\t\t\t\t<string>RunShellScriptAction</string>\n\
\t\t\t\t<key>InputUUID</key>\n\t\t\t\t<string>6B0C8A24-6F6B-4D1B-9C1A-1E2B3C4D5E61</string>\n\
\t\t\t\t<key>Keywords</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>Shell</string>\n\t\t\t\t\t<string>Script</string>\n\t\t\t\t</array>\n\
\t\t\t\t<key>OutputUUID</key>\n\t\t\t\t<string>6B0C8A24-6F6B-4D1B-9C1A-1E2B3C4D5E62</string>\n\
\t\t\t\t<key>UUID</key>\n\t\t\t\t<string>6B0C8A24-6F6B-4D1B-9C1A-1E2B3C4D5E63</string>\n\
\t\t\t\t<key>UnlocalizedApplications</key>\n\t\t\t\t<array>\n\t\t\t\t\t<string>Automator</string>\n\t\t\t\t</array>\n\
\t\t\t\t<key>arguments</key>\n\t\t\t\t<dict>\n{arguments}\t\t\t\t</dict>\n\
\t\t\t\t<key>isViewVisible</key>\n\t\t\t\t<integer>1</integer>\n\
\t\t\t\t<key>location</key>\n\t\t\t\t<string>309.000000:253.000000</string>\n\
\t\t\t\t<key>nibPath</key>\n\t\t\t\t<string>/System/Library/Automator/Run Shell Script.action/Contents/Resources/Base.lproj/main.nib</string>\n\
\t\t\t</dict>\n\t\t\t<key>isViewVisible</key>\n\t\t\t<integer>1</integer>\n\t\t</dict>\n\t</array>\n\
\t<key>connectors</key>\n\t<dict/>\n\
\t<key>workflowMetaData</key>\n\t<dict>\n\
\t\t<key>serviceInputTypeIdentifier</key>\n\t\t<string>com.apple.Automator.text</string>\n\
\t\t<key>serviceOutputTypeIdentifier</key>\n\t\t<string>com.apple.Automator.nothing</string>\n\
\t\t<key>serviceProcessesInput</key>\n\t\t<integer>0</integer>\n\
\t\t<key>workflowTypeIdentifier</key>\n\t\t<string>com.apple.Automator.servicesMenu</string>\n\
\t</dict>\n</dict>\n</plist>\n",
        command = xml(&safari_command(kobo)),
    );
    [
        ("Contents/Info.plist", info),
        ("Contents/document.wflow", workflow),
    ]
}

const USAGE: &str =
    "usage: kobo clip install --browser chrome|edge|brave|chromium|firefox|safari\n\
                     \x20      kobo clip install --all\n\
                     \x20      kobo clip uninstall [--browser NAME | --all]";

/// The browsers a command line names: one with `--browser`, or every one
/// found here with `--all`.
fn chosen(
    arguments: &[String],
    os: Os,
    home: &Path,
    default_all: bool,
) -> Result<Vec<Browser>, String> {
    let mut browsers = Vec::new();
    let mut all = false;
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--all" => all = true,
            "--browser" => {
                let name = words.next().ok_or("--browser takes a browser's name")?;
                let browser = Browser::named(name)
                    .ok_or_else(|| format!("{name} is not a browser this knows. {USAGE}"))?;
                if !browsers.contains(&browser) {
                    browsers.push(browser);
                }
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    if all {
        let local = std::env::var_os("LOCALAPPDATA").map_or_else(PathBuf::new, PathBuf::from);
        let roaming = std::env::var_os("APPDATA").map_or_else(PathBuf::new, PathBuf::from);
        browsers = Browser::ALL
            .into_iter()
            .filter(|browser| {
                let folder = profile_folder(*browser, os, home, &local, &roaming);
                !folder.as_os_str().is_empty() && folder.exists()
            })
            .collect();
        if browsers.is_empty() {
            return Err(
                "No browser this supports was found here. Name one with --browser.".to_owned(),
            );
        }
    } else if browsers.is_empty() {
        if default_all {
            return Ok(Browser::ALL.to_vec());
        }
        return Err(USAGE.to_owned());
    }
    if os != Os::Mac && browsers.contains(&Browser::Safari) {
        return Err("Safari is only on a Mac.".to_owned());
    }
    Ok(browsers)
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "There is no home folder to install into.".to_owned())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    }
    fs::write(path, bytes).map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(unix)]
fn executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("could not make {} runnable: {error}", path.display()))
}

#[cfg(not(unix))]
fn executable(_: &Path) -> Result<(), String> {
    Ok(())
}

fn reg(arguments: &[String]) -> Result<(), String> {
    let status = std::process::Command::new("reg")
        .args(arguments)
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("run reg: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reg {} failed", arguments.join(" ")))
    }
}

/// `kobo clip install`.
pub fn install(arguments: &[String]) -> Result<(), String> {
    let os = Os::current();
    let home = home()?;
    let browsers = chosen(arguments, os, &home, false)?;
    let config = delivery::config_root();
    let clip = config.join("clip");
    let kobo = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|error| format!("could not find this kobo command: {error}"))?;
    let (name, script) = launcher(os, &kobo);
    let launcher_path = clip.join(name);
    write(&launcher_path, script.as_bytes())?;
    executable(&launcher_path)?;
    let extension = clip.join("extension");
    if browsers.iter().any(|browser| *browser != Browser::Safari) {
        for (name, bytes) in EXTENSION {
            write(&extension.join(name), bytes)?;
        }
    }
    let mut said = Vec::new();
    for browser in &browsers {
        let places = registrations(*browser, os, &home, &config);
        for place in &places {
            match place {
                Registration::File(path) => {
                    write(path, host_manifest(*browser, &launcher_path).as_bytes())?;
                }
                Registration::Registry { key, manifest } => {
                    write(manifest, host_manifest(*browser, &launcher_path).as_bytes())?;
                    reg(&registry_add(key, manifest))?;
                }
            }
        }
        if *browser == Browser::Safari {
            let service = safari_service(&home);
            for (name, text) in safari_files(&kobo) {
                write(&service.join(name), text.as_bytes())?;
            }
            // Services are read when the menu is next built; this asks for
            // that now rather than at the next login.
            let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
                .arg("-update")
                .status();
        }
        said.push(steps(*browser, &extension));
    }
    let names: Vec<&str> = browsers.iter().map(|browser| browser.title()).collect();
    println!(
        "Send to Kobo is set up for {}. One step is left in each:\n",
        names.join(", ")
    );
    for step in said {
        println!("{step}\n");
    }
    println!(
        "Articles go to the reader kobo setup saved, or wait on this computer while it is asleep."
    );
    Ok(())
}

/// `kobo clip uninstall`: takes away what install added, for the browsers
/// named or for all of them. Articles still waiting are kept.
pub fn uninstall(arguments: &[String]) -> Result<(), String> {
    let os = Os::current();
    let home = home()?;
    let browsers = chosen(arguments, os, &home, true)?;
    let everything = browsers.len() == Browser::ALL.len();
    let config = delivery::config_root();
    for browser in &browsers {
        match registration(*browser, os, &home, &config) {
            Some(Registration::File(path)) => {
                let _ = fs::remove_file(path);
            }
            Some(Registration::Registry { key, manifest }) => {
                let _ = reg(&registry_delete(&key));
                let _ = fs::remove_file(manifest);
            }
            None if os == Os::Mac => {
                let _ = fs::remove_dir_all(safari_service(&home));
            }
            None => {}
        }
    }
    if everything {
        let clip = config.join("clip");
        let _ = fs::remove_dir_all(clip.join("extension"));
        let _ = fs::remove_file(clip.join(launcher(os, Path::new("kobo")).0));
        let _ = fs::remove_dir_all(clip.join("chromium"));
        let _ = fs::remove_dir_all(clip.join("firefox"));
    }
    let names: Vec<&str> = browsers.iter().map(|browser| browser.title()).collect();
    println!(
        "Send to Kobo is gone from {}. Remove the extension in the browser's own extensions page too.",
        names.join(", ")
    );
    let waiting = delivery::waiting(&delivery::queue_folder()).len();
    if waiting > 0 {
        println!(
            "{} still waiting for the reader stay in {}; kobo clip sync sends them.",
            delivery::articles_word(waiting),
            delivery::queue_folder().display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/ada")
    }

    fn config() -> PathBuf {
        PathBuf::from("/home/ada/.config/kobo")
    }

    fn file(browser: Browser, os: Os) -> String {
        match registration(browser, os, &home(), &config()) {
            Some(Registration::File(path)) => path.to_string_lossy().into_owned(),
            other => panic!("expected a file for {browser:?} on {os:?}, got {other:?}"),
        }
    }

    #[test]
    fn each_browser_on_a_mac_reads_the_manifest_from_its_own_folder() {
        let name = format!("{HOST_NAME}.json");
        let at = |folder: &str| {
            format!("/home/ada/Library/Application Support/{folder}/NativeMessagingHosts/{name}")
        };
        assert_eq!(file(Browser::Chrome, Os::Mac), at("Google/Chrome"));
        assert_eq!(file(Browser::Edge, Os::Mac), at("Microsoft Edge"));
        assert_eq!(
            file(Browser::Brave, Os::Mac),
            at("BraveSoftware/Brave-Browser")
        );
        assert_eq!(file(Browser::Chromium, Os::Mac), at("Chromium"));
        assert_eq!(file(Browser::Firefox, Os::Mac), at("Mozilla"));
        assert_eq!(
            registration(Browser::Safari, Os::Mac, &home(), &config()),
            None
        );
    }

    #[test]
    fn each_browser_on_linux_reads_the_manifest_from_its_own_folder() {
        let name = format!("{HOST_NAME}.json");
        assert_eq!(
            file(Browser::Chrome, Os::Linux),
            format!("/home/ada/.config/google-chrome/NativeMessagingHosts/{name}")
        );
        assert_eq!(
            file(Browser::Edge, Os::Linux),
            format!("/home/ada/.config/microsoft-edge/NativeMessagingHosts/{name}")
        );
        assert_eq!(
            file(Browser::Brave, Os::Linux),
            format!("/home/ada/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts/{name}")
        );
        assert_eq!(
            file(Browser::Chromium, Os::Linux),
            format!("/home/ada/.config/chromium/NativeMessagingHosts/{name}")
        );
        assert_eq!(
            file(Browser::Firefox, Os::Linux),
            format!("/home/ada/.mozilla/native-messaging-hosts/{name}")
        );
    }

    #[test]
    fn on_windows_each_browser_gets_a_per_user_key_naming_a_kept_manifest() {
        let expect = |browser, vendor: &str, family: &str| {
            assert_eq!(
                registration(browser, Os::Windows, &home(), &config()),
                Some(Registration::Registry {
                    key: format!(r"HKCU\Software\{vendor}\NativeMessagingHosts\{HOST_NAME}"),
                    manifest: config()
                        .join("clip")
                        .join(family)
                        .join(format!("{HOST_NAME}.json")),
                })
            );
        };
        expect(Browser::Chrome, r"Google\Chrome", "chromium");
        expect(Browser::Edge, r"Microsoft\Edge", "chromium");
        expect(Browser::Brave, r"BraveSoftware\Brave-Browser", "chromium");
        expect(Browser::Chromium, "Chromium", "chromium");
        expect(Browser::Firefox, "Mozilla", "firefox");
        let key = r"HKCU\Software\Google\Chrome\NativeMessagingHosts\x";
        assert_eq!(
            registry_add(key, Path::new(r"C:\Users\ada\kobo\x.json")),
            [
                "add",
                key,
                "/ve",
                "/t",
                "REG_SZ",
                "/d",
                r"C:\Users\ada\kobo\x.json",
                "/f"
            ]
        );
        assert_eq!(registry_delete(key), ["delete", key, "/f"]);
    }

    fn parsed(json: &str) -> kobo_json::Value {
        kobo_json::parse(json).expect("a manifest is JSON")
    }

    #[test]
    fn brave_is_registered_where_it_was_seen_reading_as_well_as_its_own_folder() {
        let places = registrations(Browser::Brave, Os::Mac, &home(), &config());
        assert_eq!(
            places,
            vec![
                registration(Browser::Brave, Os::Mac, &home(), &config()).expect("brave's"),
                registration(Browser::Chrome, Os::Mac, &home(), &config()).expect("chrome's"),
            ]
        );
        assert_eq!(
            registrations(Browser::Edge, Os::Linux, &home(), &config()).len(),
            1
        );
        assert!(registrations(Browser::Safari, Os::Mac, &home(), &config()).is_empty());
    }

    #[test]
    fn a_chromium_manifest_lets_in_only_the_pinned_extension() {
        let manifest = parsed(&host_manifest(
            Browser::Chrome,
            Path::new("/opt/kobo-clip-host"),
        ));
        let text = |key: &str| {
            manifest
                .get(key)
                .and_then(kobo_json::Value::as_str)
                .map(str::to_owned)
        };
        assert_eq!(text("name").as_deref(), Some(HOST_NAME));
        assert_eq!(text("type").as_deref(), Some("stdio"));
        assert_eq!(text("path").as_deref(), Some("/opt/kobo-clip-host"));
        let origins = manifest
            .get("allowed_origins")
            .and_then(kobo_json::Value::as_array)
            .expect("origins");
        assert_eq!(origins.len(), 1);
        assert_eq!(
            origins[0].as_str(),
            Some("chrome-extension://oddgeegniblkiahbnpjbgjoafeanokgi/")
        );
        assert!(manifest.get("allowed_extensions").is_none());
    }

    #[test]
    fn a_firefox_manifest_names_the_extension_by_its_id() {
        let manifest = parsed(&host_manifest(
            Browser::Firefox,
            Path::new("/opt/kobo-clip-host"),
        ));
        let ids = manifest
            .get("allowed_extensions")
            .and_then(kobo_json::Value::as_array)
            .expect("extensions");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].as_str(), Some(FIREFOX_ID));
        assert!(manifest.get("allowed_origins").is_none());
    }

    #[test]
    fn the_ids_here_are_the_ones_the_extension_manifest_pins() {
        let manifest = std::str::from_utf8(EXTENSION[0].1).expect("text");
        let manifest = parsed(manifest);
        let key = manifest
            .get("key")
            .and_then(kobo_json::Value::as_str)
            .expect("a pinned key");
        let der = super::super::frame::base64_decode(key).expect("base64");
        let digest = super::super::sha256::hex_digest(&der);
        let id: String = digest[..32]
            .chars()
            .map(|hex| {
                char::from(b'a' + u8::try_from(hex.to_digit(16).expect("hex")).expect("small"))
            })
            .collect();
        assert_eq!(id, CHROMIUM_ID);
        let gecko = manifest
            .get("browser_specific_settings")
            .and_then(|settings| settings.get("gecko"))
            .and_then(|gecko| gecko.get("id"))
            .and_then(kobo_json::Value::as_str);
        assert_eq!(gecko, Some(FIREFOX_ID));
    }

    #[test]
    fn the_launcher_runs_the_native_host_with_whatever_the_browser_passes() {
        let (name, script) = launcher(Os::Mac, Path::new("/Users/ada/bin/it's kobo"));
        assert_eq!(name, "kobo-clip-host");
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains(r"exec '/Users/ada/bin/it'\''s kobo' clip native-host "));
        assert!(script.trim_end().ends_with("\"$@\""));
        let (name, script) = launcher(Os::Windows, Path::new(r"C:\Program Files\Cobalt\kobo.exe"));
        assert_eq!(name, "kobo-clip-host.bat");
        assert_eq!(
            script,
            "@echo off\r\n\"C:\\Program Files\\Cobalt\\kobo.exe\" clip native-host %*\r\n"
        );
    }

    #[test]
    fn each_browser_is_told_exactly_what_to_click() {
        let folder = Path::new("/home/ada/.config/kobo/clip/extension");
        let chrome = steps(Browser::Chrome, folder);
        assert!(chrome.contains("chrome://extensions"));
        assert!(chrome.contains("Developer mode"));
        assert!(chrome.contains("Load unpacked"));
        assert!(chrome.contains("/home/ada/.config/kobo/clip/extension"));
        assert!(steps(Browser::Edge, folder).contains("edge://extensions"));
        assert!(steps(Browser::Brave, folder).contains("brave://extensions"));
        let firefox = steps(Browser::Firefox, folder);
        assert!(firefox.contains("about:debugging"));
        assert!(firefox.contains("Load Temporary Add-on"));
        assert!(firefox.contains("manifest.json"));
        assert!(firefox.contains("restarts"));
        let safari = steps(Browser::Safari, folder);
        assert!(safari.contains("Services"));
        assert!(safari.contains("signed Mac app"));
    }

    #[test]
    fn the_safari_service_clips_each_address_it_is_given_and_nothing_else() {
        let command = safari_command(Path::new("/opt/kobo"));
        assert!(command.contains("http://*|https://*) '/opt/kobo' clip \"$page\" --notify"));
        let [(info_name, info), (flow_name, flow)] = safari_files(Path::new("/opt/a&b/kobo"));
        assert_eq!(info_name, "Contents/Info.plist");
        assert!(info.contains("<string>Send to Kobo</string>"));
        assert!(info.contains("public.url"));
        assert_eq!(flow_name, "Contents/document.wflow");
        assert!(
            flow.contains("/opt/a&amp;b/kobo"),
            "the command is escaped for the plist"
        );
        assert!(flow.contains("com.apple.Automator.servicesMenu"));
    }

    #[test]
    fn a_browser_is_named_or_found_and_safari_only_on_a_mac() {
        let words = |text: &str| {
            text.split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            chosen(
                &words("--browser Firefox --browser chrome"),
                Os::Linux,
                &home(),
                false
            ),
            Ok(vec![Browser::Firefox, Browser::Chrome])
        );
        assert!(chosen(&words("--browser safari"), Os::Linux, &home(), false).is_err());
        assert!(chosen(&words("--browser opera"), Os::Mac, &home(), false).is_err());
        assert!(
            chosen(&[], Os::Mac, &home(), false).is_err(),
            "install names one"
        );
        assert_eq!(
            chosen(&[], Os::Mac, &home(), true).map(|all| all.len()),
            Ok(Browser::ALL.len()),
            "uninstall with nothing named takes away every one"
        );
    }

    #[test]
    fn a_browser_counts_as_installed_where_it_keeps_its_profiles() {
        let none = Path::new("");
        assert_eq!(
            profile_folder(Browser::Chrome, Os::Linux, &home(), none, none),
            home().join(".config/google-chrome")
        );
        assert_eq!(
            profile_folder(
                Browser::Edge,
                Os::Windows,
                &home(),
                Path::new(r"C:\Users\ada\AppData\Local"),
                none
            ),
            Path::new(r"C:\Users\ada\AppData\Local").join(r"Microsoft\Edge\User Data")
        );
        assert_eq!(
            profile_folder(Browser::Safari, Os::Mac, &home(), none, none),
            PathBuf::from("/Applications/Safari.app")
        );
    }
}
