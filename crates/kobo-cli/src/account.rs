//! `kobo connect APP`: get an account app connected from this computer.
//!
//! Typing a server address and a 40-character token on an e-ink keyboard is
//! the slowest and least reliable step in setting up any account app, so the
//! whole of it happens here, where there is a clipboard. The owner names the
//! app; this asks for the server and the token, checks them against the
//! server before anything is installed, and delivers both the credential and
//! the app's own server setting to the reader in one SSH session. The app
//! picks them up the next time it comes to the front, so nothing on the
//! reader has to be retyped or restarted.
//!
//! Post's `kobo post login` was the model: check first, pin the token to the
//! one server it was checked against, write the app's setting, deliver.

use std::fs;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const USAGE: &str = "usage: kobo connect APP [--server URL] [--token-file PATH | --stdin] [--username NAME]\n\
                     \x20                    [--ca ROOT.pem] [--sim | --device IP | --reader NAME]\n\
                     \n\
                     Connects an account app from this computer. Asks for the server and the\n\
                     token, checks them against the server, then installs the token on the\n\
                     reader and saves the server in the app, so nothing is typed on the reader.\n\
                     Run it again to replace a token that expired.\n\
                     \n\
                     Apps: readeck, calibre-web (Library), rss-miniflux (Digest), readlater\n\
                     (Read Later), kitchencard (Kitchen Card), homepanel (Home Panel),\n\
                     zotero-reader (Stacks), post.\n\
                     \n\
                     --ca trusts a private certificate authority for the check, for a home\n\
                     server with its own certificate. Install the same root on the reader with\n\
                     kobo trust set.";

/// Where the reader keeps everything Cobalt owns.
pub(crate) const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt";

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);

/// How the token is sent to the service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Auth {
    Bearer,
    /// `username:password`, sent as HTTP Basic.
    Basic,
    Header(&'static str),
}

/// Where the runtime keeps the credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Holding {
    /// Bound to the one server it was checked against, readable only by this
    /// app. The runtime refuses to send it anywhere else.
    Bound,
    /// The owner's copy under a plain name, as `kobo secret set` installs it.
    Owner,
}

/// One account app this command knows how to connect.
#[derive(Debug)]
pub(crate) struct Account {
    pub app: &'static str,
    pub title: &'static str,
    pub service: &'static str,
    aliases: &'static [&'static str],
    pub secret: &'static str,
    pub holding: Holding,
    pub auth: Auth,
    /// The service's one address, for a service the owner does not host.
    fixed_server: Option<&'static str>,
    /// What is asked for, after the server is known, to prove the token.
    probe: &'static str,
    /// Where the owner finds the token, said when asking for it.
    token_hint: &'static str,
    server_example: &'static str,
}

pub(crate) const ACCOUNTS: &[Account] = &[
    Account {
        app: "readeck",
        title: "Readeck",
        service: "Readeck",
        aliases: &[],
        secret: "readeck",
        holding: Holding::Bound,
        auth: Auth::Bearer,
        fixed_server: None,
        probe: "/api/profile",
        token_hint: "Create one in Readeck under Settings, API tokens.",
        server_example: "https://readeck.example",
    },
    Account {
        app: "calibre-web",
        title: "Library",
        service: "calibre-web",
        aliases: &["library", "calibre"],
        secret: "calibre",
        holding: Holding::Bound,
        auth: Auth::Basic,
        fixed_server: None,
        probe: "",
        token_hint: "Use your calibre-web username and password.",
        server_example: "https://books.example/opds",
    },
    Account {
        app: "rss-miniflux",
        title: "Digest",
        service: "Miniflux",
        aliases: &["digest", "miniflux"],
        secret: "miniflux",
        holding: Holding::Bound,
        auth: Auth::Header("X-Auth-Token"),
        fixed_server: None,
        probe: "/v1/me",
        token_hint: "Create one in Miniflux under Settings, API Keys.",
        server_example: "https://miniflux.example",
    },
    Account {
        app: "kitchencard",
        title: "Kitchen Card",
        service: "Mealie",
        aliases: &["kitchen-card", "mealie"],
        secret: "mealie",
        holding: Holding::Owner,
        auth: Auth::Bearer,
        fixed_server: None,
        probe: "/api/users/self",
        token_hint: "Create one in Mealie under your profile, API Tokens.",
        server_example: "https://mealie.example",
    },
    Account {
        app: "homepanel",
        title: "Home Panel",
        service: "Home Assistant",
        aliases: &["home-panel", "homeassistant", "home-assistant"],
        secret: "homeassistant",
        holding: Holding::Owner,
        auth: Auth::Bearer,
        fixed_server: None,
        probe: "/api/",
        token_hint: "Create a long-lived access token on your Home Assistant profile page.",
        server_example: "https://homeassistant.example:8123",
    },
    Account {
        app: "zotero-reader",
        title: "Stacks",
        service: "Zotero",
        aliases: &["stacks", "zotero"],
        secret: "zotero",
        holding: Holding::Owner,
        auth: Auth::Bearer,
        fixed_server: Some("https://api.zotero.org"),
        probe: "/keys/current",
        token_hint: "Create a read-only key at zotero.org/settings/keys.",
        server_example: "https://api.zotero.org",
    },
    Account {
        app: "post",
        title: "Post",
        service: "Hermes",
        aliases: &["hermes"],
        secret: "hermes-post",
        holding: Holding::Bound,
        auth: Auth::Bearer,
        fixed_server: None,
        probe: "/letters?page=1&per_page=1",
        token_hint: "Use the bearer token your Hermes gateway prints.",
        server_example: "https://letters.example",
    },
];

/// Read Later signs in with Wallabag's OAuth grant rather than a token, so it
/// goes through `kobo readlater login`'s own flow.
const READ_LATER: &[&str] = &["readlater", "read-later", "wallabag"];

/// The app a name the owner typed refers to: its id, its title or its
/// service, in any case.
pub(crate) fn find(name: &str) -> Option<&'static Account> {
    let name = name.trim().to_ascii_lowercase().replace(' ', "-");
    ACCOUNTS.iter().find(|account| {
        account.app == name
            || account.title.to_ascii_lowercase().replace(' ', "-") == name
            || account.aliases.contains(&name.as_str())
    })
}

/// The account a server-bound `kobo secret set` names, if this command knows
/// how to check it.
pub(crate) fn for_secret(app: &str, secret: &str) -> Option<&'static Account> {
    ACCOUNTS
        .iter()
        .find(|account| account.app == app && account.secret == secret)
}

/// Where the reader is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Destination {
    Sim,
    Device(String),
}

impl Account {
    /// The address the token is checked against and the app's own setting is
    /// built from. calibre-web's is the OPDS catalog, the others are the
    /// server's base address.
    pub(crate) fn server(&self, typed: &str) -> Result<String, String> {
        if let Some(fixed) = self.fixed_server {
            return Ok(fixed.to_owned());
        }
        let typed = typed.trim().trim_end_matches('/');
        let typed = if typed.contains("://") {
            typed.to_owned()
        } else {
            format!("https://{typed}")
        };
        if !kobo_policy::credentials::servers::valid_server(&typed) || typed.contains('%') {
            return Err(format!(
                "{typed} is not an address Cobalt can use. Use an HTTPS address such as {}, without a username, password, query or fragment.",
                self.server_example
            ));
        }
        if self.app == "calibre-web" {
            return Ok(opds_endpoint(&typed));
        }
        Ok(typed)
    }

    /// The server the runtime binds the token to. The origin for
    /// calibre-web, because book downloads live beside the catalog rather
    /// than under it, and the app binds its own account the same way.
    pub(crate) fn binding(&self, server: &str) -> String {
        if self.app == "calibre-web" {
            origin(server)
        } else {
            server.to_owned()
        }
    }

    fn probe_url(&self, server: &str) -> String {
        format!("{server}{}", self.probe)
    }

    fn header(&self, value: &str) -> (&'static str, String) {
        match self.auth {
            Auth::Bearer => ("Authorization", format!("Bearer {value}")),
            Auth::Basic => (
                "Authorization",
                format!("Basic {}", super::base64_encode(value.as_bytes())),
            ),
            Auth::Header(name) => (name, value.to_owned()),
        }
    }

    /// The app's own settings, as it reads them back from its state.
    ///
    /// Each value is in exactly the format the app writes when the owner
    /// types it on the reader, so the app cannot tell the difference.
    pub(crate) fn state(
        &self,
        server: &str,
        answer: &[u8],
    ) -> Result<Vec<(&'static str, String)>, String> {
        Ok(match self.app {
            "readeck" => vec![("server", server.to_owned())],
            "calibre-web" => vec![("catalog", format!("{server}|{}", self.secret))],
            "rss-miniflux" | "kitchencard" => {
                vec![("config", format!("{server}\n{}", self.secret))]
            }
            "homepanel" => vec![("base-url", server.to_owned())],
            "post" => vec![("gateway", server.to_owned())],
            "zotero-reader" => vec![("zotero-user-id", zotero_user(answer)?)],
            _ => Vec::new(),
        })
    }
}

/// `https://host/` becomes `https://host/opds`; an address with a path is
/// kept, the same rule the app applies to a typed address.
fn opds_endpoint(server: &str) -> String {
    let after = server.split_once("://").map_or(server, |(_, rest)| rest);
    if after.contains('/') {
        server.to_owned()
    } else {
        format!("{server}/opds")
    }
}

fn origin(server: &str) -> String {
    server.split('/').take(3).collect::<Vec<_>>().join("/")
}

/// The numeric user ID Zotero reports for a key, which Stacks needs and the
/// owner would otherwise copy by eye from the key settings page.
fn zotero_user(answer: &[u8]) -> Result<String, String> {
    let value = std::str::from_utf8(answer)
        .ok()
        .and_then(|text| kobo_json::parse(text).ok())
        .ok_or("Zotero's answer was not readable. Try again.")?;
    let id = value
        .get("userID")
        .and_then(|id| id.as_i64().map(|id| id.to_string()).or_else(|| id.as_str().map(str::to_owned)))
        .filter(|id| !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("Zotero did not say which user this key belongs to. Create a key for your personal library.")?;
    Ok(id)
}

/// Checks a token against its server and returns what the server answered.
///
/// Nothing reaches the reader unless this succeeds. A token that does not
/// open the service would be installed and then fail in the owner's hands,
/// on a screen with far less room to explain why.
pub(crate) fn check(account: &Account, server: &str, value: &str) -> Result<Vec<u8>, String> {
    trust_local_roots();
    let (name, header) = account.header(value);
    let url = account.probe_url(server);
    let accept = [(
        "Accept",
        "application/json, application/atom+xml;q=0.9, */*;q=0.5",
    )];
    let cancel = AtomicBool::new(false);
    let answer = kobo_net::fetch_from_with_refusals(
        &url,
        0,
        256 * 1024,
        Some((name, &header)),
        &accept,
        kobo_net::RequestOptions::default(),
        &cancel,
    )
    .map_err(|error| failure(account, server, error))?;
    if let Some(refusal) = kobo_net::Refusal::parse_envelope(&answer) {
        return Err(refused(account, server, &refusal));
    }
    Ok(answer)
}

/// Routes a `--sim` check to the same fixture the app under `kobo dev`
/// reaches, so the token is checked against what the app will talk to.
///
/// # Errors
/// Refuses when a fixture was asked for and could not be set up, rather than
/// checking the token against the real service instead.
pub(crate) fn prepare_sim() -> Result<(), String> {
    if kobo_sim::prepare_simulated_network() {
        Ok(())
    } else {
        Err("The simulator's network fixture could not be set up, so nothing was checked or installed.".to_owned())
    }
}

/// The owner's private roots, as the simulator loads them, so a home server
/// with its own certificate authority can be checked from here too.
fn trust_local_roots() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let directory = std::env::var_os("KOBO_SIM_TRUST_DIR").map_or_else(
            || {
                std::env::var_os("HOME").map_or_else(
                    || PathBuf::from(".kobo-trust"),
                    |home| {
                        PathBuf::from(home)
                            .join(".config")
                            .join("kobo")
                            .join("trust")
                    },
                )
            },
            PathBuf::from,
        );
        let _ = kobo_net::trust_owner_roots_from_dir(&directory);
    });
}

/// What to say when the service refused the check, in its own terms.
pub(crate) fn refused(account: &Account, server: &str, refusal: &kobo_net::Refusal) -> String {
    let service = account.service;
    let what = if account.auth == Auth::Basic {
        "username or password"
    } else {
        "token"
    };
    match refusal.status {
        401 => format!(
            "{service} at {server} did not accept the {what}. It may be mistyped, expired or revoked. {} Nothing was installed.",
            account.token_hint
        ),
        403 => format!(
            "{service} at {server} accepted the {what} but will not let it read your account. Give it more access, or create one that has it. Nothing was installed."
        ),
        404 | 405 | 410 => format!(
            "{server} answered, but no {service} was found there. Check the address, including any path. Nothing was installed."
        ),
        429 => format!(
            "{service} at {server} asked this computer to slow down. Wait a minute and run this again. Nothing was installed."
        ),
        status => format!(
            "{service} at {server} refused the check (HTTP {status}). Nothing was installed."
        ),
    }
}

/// What to say when the check could not reach the service.
pub(crate) fn failure(account: &Account, server: &str, error: kobo_protocol::TaskError) -> String {
    use kobo_protocol::TaskError;
    let service = account.service;
    match error {
        TaskError::Offline => {
            "This computer is not on a network. Connect it and run this again. Nothing was installed.".to_owned()
        }
        TaskError::Unreachable => format!(
            "{service} at {server} did not answer, or its certificate is not trusted. Check the address and that the server is running. For a server with its own certificate authority, add --ca ROOT.pem here and install the same root on the reader with kobo trust set. Nothing was installed."
        ),
        TaskError::TimedOut => format!(
            "{service} at {server} took too long to answer. Run this again when it is reachable. Nothing was installed."
        ),
        TaskError::Unauthorized => refused(
            account,
            server,
            &kobo_net::Refusal {
                status: 401,
                retry_after: None,
                excerpt: String::new(),
            },
        ),
        TaskError::RateLimited(_) => refused(
            account,
            server,
            &kobo_net::Refusal {
                status: 429,
                retry_after: None,
                excerpt: String::new(),
            },
        ),
        TaskError::NotFound => format!(
            "{server} answered, but no {service} was found there. Check the address, including any path. Nothing was installed."
        ),
        TaskError::TooLarge | TaskError::Denied | TaskError::NoCredential => format!(
            "The check against {service} at {server} failed. Nothing was installed."
        ),
    }
}

/// One credential to install: whose it is, under which name, how the runtime
/// holds it, and the server it is bound to.
pub(crate) struct Install<'a> {
    pub app: &'a str,
    pub secret: &'a str,
    pub holding: Holding,
    /// The server a bound account may be sent to. Unused for an owner copy.
    pub server: &'a str,
    pub value: &'a str,
}

impl Account {
    pub(crate) fn install<'a>(&'a self, binding: &'a str, value: &'a str) -> Install<'a> {
        Install {
            app: self.app,
            secret: self.secret,
            holding: self.holding,
            server: binding,
            value,
        }
    }
}

/// Installs the credential and the app's settings, in one step per reader.
///
/// # Errors
/// Reports a reader that could not be reached or refused the write. The
/// previous account is left in place on any failure.
pub(crate) fn deliver(
    install: &Install<'_>,
    destination: &Destination,
    state: &[(&'static str, String)],
) -> Result<(), String> {
    match destination {
        Destination::Sim => deliver_sim(
            &kobo_sim::simulated_secrets_root(),
            &kobo_sim::simulated_state_root(install.app),
            install,
            state,
        ),
        Destination::Device(host) => {
            let script = device_script(DEVICE_ROOT, install, state);
            let output =
                super::run_remote_shell(&format!("root@{host}"), &script, TRANSFER_TIMEOUT)
                    .map_err(super::unreachable_device)?;
            if output.status.success() && output.stdout.ends_with(b"ACCOUNT_CONNECTED\n") {
                return Ok(());
            }
            // Remote diagnostics can echo the script, and the script holds
            // the token. Only the exit code is ever reported.
            Err(match output.status.code() {
                Some(20) => "Cobalt is finishing an update on the reader. Wait a minute and run this again.".to_owned(),
                Some(21) => "The reader's credential folder is not safe to write (a link or an unexpected file). Nothing was changed.".to_owned(),
                _ => format!(
                    "The account was not confirmed on {host}. Check the reader is awake and on Wi-Fi, then run this again."
                ),
            })
        }
    }
}

/// The simulator keeps the reader's folders on this computer: `secrets` is
/// its credential store and `directory` holds this app's settings.
fn deliver_sim(
    secrets: &Path,
    directory: &Path,
    install: &Install<'_>,
    state: &[(&'static str, String)],
) -> Result<(), String> {
    match install.holding {
        Holding::Bound => kobo_policy::credentials::servers::install(
            secrets,
            install.app,
            install.secret,
            install.server,
            install.value,
        )
        .map_err(|error| format!("save the simulator's account: {error:?}"))?,
        Holding::Owner => {
            fs::create_dir_all(secrets)
                .map_err(|error| format!("create {}: {error}", secrets.display()))?;
            let path = secrets.join(install.secret);
            let _ = fs::remove_file(secrets.join(format!(".{}.writing", install.secret)));
            super::publish_secret(&path, install.value)?;
            super::forget_typed_copies(secrets, install.secret)?;
        }
    }
    fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    for (key, value) in state {
        let partial = directory.join(format!(".{key}.writing"));
        fs::write(&partial, value)
            .map_err(|error| format!("write {}: {error}", partial.display()))?;
        fs::rename(&partial, directory.join(key))
            .map_err(|error| format!("save {}: {error}", directory.display()))?;
    }
    Ok(())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// One shell script that installs the credential and then the settings.
///
/// The credential goes first: an app that finds its server but not its key
/// asks for the key, while one that finds a key and an old server would
/// spend the key on the old server. Every file is staged beside its final
/// name and renamed into place, so a dropped connection leaves the previous
/// account whole. Cobalt may be running: the runtime reads an account when a
/// request needs it, and an app reads its settings when it comes back to the
/// front, so neither has a copy to fall out of step with. Only an update in
/// progress is waited out, because an update restores the owner's folders
/// if it rolls back.
pub(crate) fn device_script(
    root: &str,
    install: &Install<'_>,
    state: &[(&'static str, String)],
) -> String {
    let credential = match install.holding {
        Holding::Bound => bound_script(install),
        Holding::Owner => super::secret_install_script(install.secret, install.value)
            .replace(super::DEVICE_SECRETS_DIRECTORY, &format!("{root}/secrets")),
    };
    let mut settings = String::new();
    for (key, value) in state {
        settings.push_str(&format!(
            "partial=\"$state/.{key}.writing\"\n\
             base64 -d > \"$partial\" <<'COBALT_APP_SETTING'\n{}\nCOBALT_APP_SETTING\n\
             chmod 600 \"$partial\"\n\
             mv -f \"$partial\" \"$state/{key}\"\n",
            super::base64_encode(value.as_bytes())
        ));
    }
    format!(
        "set -eu\nset +x\numask 077\nroot={root}\napp={app}\n\
         if command -v pidof >/dev/null 2>&1 && pidof recovery-kobod >/dev/null 2>&1; then exit 20; fi\n\
         (\n{credential}\n)\n\
         state=\"$root/state/$app\"\n\
         [ ! -L \"$root/state\" ] && [ ! -L \"$state\" ] || exit 21\n\
         mkdir -p \"$state\"\nchmod 700 \"$state\"\n\
         {settings}sync\nprintf 'ACCOUNT_CONNECTED\\n'\n",
        root = quote(root),
        app = quote(install.app),
    )
}

/// The server-bound record, in the runtime's own format, staged under a lock
/// so two transfers never interleave.
fn bound_script(install: &Install<'_>) -> String {
    let record = super::base64_encode(
        format!(
            "cobalt-server-account-v1\n{}\n{}",
            install.server, install.value
        )
        .as_bytes(),
    );
    format!(
        r#"set -eu
p="$root"
while [ "$p" != / ]; do
  [ ! -L "$p" ] && [ -d "$p" ] || exit 21
  p=$(dirname "$p")
done
for suffix in secrets secrets/apps "secrets/apps/$app" "secrets/apps/$app/servers"; do
  p="$root/$suffix"
  [ ! -L "$p" ] || exit 21
  if [ -e "$p" ]; then [ -d "$p" ] || exit 21; else mkdir "$p"; fi
  chmod 700 "$p"
done
account="$root/secrets/apps/$app/servers/"{name}
[ ! -L "$account" ] || exit 21
if [ -e "$account" ]; then [ -f "$account" ] || exit 21; fi
lock="$root/.server-account-setup.lock"
mkdir "$lock" 2>/dev/null || exit 21
trap 'rm -rf "$lock"' EXIT
trap 'exit 22' HUP INT TERM
base64 -d > "$lock/new-account" <<'COBALT_SERVER_ACCOUNT'
{record}
COBALT_SERVER_ACCOUNT
chmod 600 "$lock/new-account"
sync
mv -f "$lock/new-account" "$account""#,
        name = quote(install.secret),
    )
}

struct Options {
    account: &'static Account,
    server: Option<String>,
    source: Source,
    username: Option<String>,
    ca: Option<PathBuf>,
    destination: Option<Destination>,
    reader: Option<String>,
}

enum Source {
    Ask,
    File(PathBuf),
    Stdin,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    let Some((name, rest)) = arguments.split_first() else {
        return Err(USAGE.to_owned());
    };
    if READ_LATER.contains(&name.to_ascii_lowercase().as_str()) {
        return super::readlater::connect(rest);
    }
    let Some(account) = find(name) else {
        return Err(format!(
            "kobo connect does not know an app called {name}. It connects: {}, readlater.",
            ACCOUNTS
                .iter()
                .map(|account| account.app)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    let options = parse(account, rest)?;
    connect(&options)
}

fn parse(account: &'static Account, arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        account,
        server: None,
        source: Source::Ask,
        username: None,
        ca: None,
        destination: None,
        reader: None,
    };
    let mut rest = arguments.iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| USAGE.to_owned());
        match flag.as_str() {
            "--sim" if options.destination.is_none() => {
                options.destination = Some(Destination::Sim);
            }
            flag if super::is_device_flag(flag) && options.destination.is_none() => {
                let host = value()?;
                if !super::valid_device_host(&host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                options.destination = Some(Destination::Device(host));
            }
            "--reader" if options.reader.is_none() => options.reader = Some(value()?),
            "--server" if options.server.is_none() => options.server = Some(value()?),
            "--username" if options.username.is_none() => options.username = Some(value()?),
            "--ca" if options.ca.is_none() => options.ca = Some(PathBuf::from(value()?)),
            "--token-file" | "--password-file" if matches!(options.source, Source::Ask) => {
                options.source = Source::File(PathBuf::from(value()?));
            }
            "--stdin" if matches!(options.source, Source::Ask) => options.source = Source::Stdin,
            _ => return Err(USAGE.to_owned()),
        }
    }
    if options.reader.is_some() && options.destination.is_some() {
        return Err("Choose one reader: --sim, --device IP or --reader NAME.".to_owned());
    }
    Ok(options)
}

/// The reader named on the command line, or the one `kobo setup` saved.
pub(crate) fn destination(
    named: Option<Destination>,
    reader: Option<&str>,
) -> Result<Destination, String> {
    if let Some(destination) = named {
        return Ok(destination);
    }
    if let Some(name) = reader {
        return super::targets::resolve_nickname(name).map(Destination::Device);
    }
    match super::targets::configured_default() {
        Some(super::targets::DefaultReader::Address(address)) => Ok(Destination::Device(address)),
        Some(super::targets::DefaultReader::Nickname(name)) => {
            super::targets::resolve_nickname(&name).map(Destination::Device)
        }
        None => Err(
            "No reader is saved. Run kobo setup with the reader plugged in over USB, or add --device IP or --sim."
                .to_owned(),
        ),
    }
}

fn connect(options: &Options) -> Result<(), String> {
    let account = options.account;
    if let Some(path) = &options.ca {
        trust_root(path)?;
    }
    let destination = destination(options.destination.clone(), options.reader.as_deref())?;
    if destination == Destination::Sim {
        prepare_sim()?;
    }
    let interactive = std::io::stdin().is_terminal();
    let typed = match (&options.server, account.fixed_server) {
        (_, Some(fixed)) => fixed.to_owned(),
        (Some(server), None) => server.clone(),
        (None, None) if interactive => ask(&format!(
            "{} server address (for example {}): ",
            account.service, account.server_example
        ))?,
        (None, None) => {
            return Err(format!(
                "Add --server URL, the address of your {} server.",
                account.service
            ))
        }
    };
    let server = account.server(&typed)?;
    let secret = if account.auth == Auth::Basic {
        let username = match &options.username {
            Some(username) => username.clone(),
            None if interactive => ask(&format!("{} username: ", account.service))?,
            None => return Err("Add --username NAME.".to_owned()),
        };
        if username.is_empty() || username.contains(':') || username.chars().any(char::is_control) {
            return Err("Use a username without a colon or line break.".to_owned());
        }
        let password = read_value(
            &options.source,
            interactive,
            &format!("{} password: ", account.service),
        )?;
        format!("{username}:{password}")
    } else {
        if interactive && matches!(options.source, Source::Ask) {
            eprintln!("{}", account.token_hint);
        }
        read_value(
            &options.source,
            interactive,
            &format!("{} token: ", account.service),
        )?
    };
    let answer = check(account, &server, &secret)?;
    let state = account.state(&server, &answer)?;
    let binding = account.binding(&server);
    deliver(&account.install(&binding, &secret), &destination, &state)?;
    let place = match &destination {
        Destination::Sim => "the simulator".to_owned(),
        Destination::Device(host) => host.clone(),
    };
    println!(
        "Connected {} to {} at {server} on {place}. Open {} on the reader, or tap Check again if it is already open.",
        account.title, account.service, account.title
    );
    Ok(())
}

fn trust_root(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let certificates = kobo_net::pem::certificates(&String::from_utf8_lossy(&bytes));
    let certificates = if certificates.is_empty() {
        vec![bytes]
    } else {
        certificates
    };
    for certificate in certificates {
        kobo_net::trust_owner_root(certificate).map_err(|_| {
            format!(
                "{} is not a certificate this command can trust.",
                path.display()
            )
        })?;
    }
    Ok(())
}

/// Asks one visible question at the terminal.
pub(crate) fn ask(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("read the answer: {error}"))?;
    Ok(line.trim().to_owned())
}

/// The token or password, from a file, from standard input, or typed at the
/// terminal without being shown.
fn read_value(source: &Source, interactive: bool, prompt: &str) -> Result<String, String> {
    let value = match source {
        Source::File(path) => {
            let metadata = fs::symlink_metadata(path)
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            if !metadata.file_type().is_file() || metadata.len() > 4096 {
                return Err(
                    "The token file must be a regular file no larger than 4 KiB.".to_owned(),
                );
            }
            fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?
        }
        Source::Stdin => {
            let mut text = String::new();
            std::io::stdin()
                .lock()
                .take(4097)
                .read_to_string(&mut text)
                .map_err(|error| format!("read the token: {error}"))?;
            text
        }
        Source::Ask if interactive => hidden(prompt)?,
        Source::Ask => {
            return Err(
                "Add --token-file PATH or --stdin when this is not run at a terminal.".to_owned(),
            )
        }
    };
    let value = value.trim().to_owned();
    if value.is_empty()
        || value.len() > kobo_protocol::MAX_APP_SECRET_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "The token must be 1 to {} characters on one line.",
            kobo_protocol::MAX_APP_SECRET_BYTES
        ));
    }
    Ok(value)
}

/// Reads a line with the terminal's echo off, so a token pasted here is not
/// left on the screen or in the scrollback.
pub(crate) fn hidden(prompt: &str) -> Result<String, String> {
    let stty = |argument: &str| {
        std::process::Command::new("stty")
            .arg(argument)
            .stdin(std::process::Stdio::inherit())
            .status()
            .is_ok_and(|status| status.success())
    };
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let quiet = stty("-echo");
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if quiet {
        stty("echo");
    }
    eprintln!();
    read.map_err(|error| format!("read the token: {error}"))?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str) -> &'static Account {
        find(name).unwrap_or_else(|| panic!("{name} is known"))
    }

    #[test]
    fn an_app_is_found_by_its_id_its_title_or_its_service() {
        for (typed, app) in [
            ("readeck", "readeck"),
            ("Library", "calibre-web"),
            ("digest", "rss-miniflux"),
            ("Kitchen Card", "kitchencard"),
            ("home-panel", "homepanel"),
            ("Stacks", "zotero-reader"),
            ("post", "post"),
        ] {
            assert_eq!(account(typed).app, app, "{typed}");
        }
        assert!(find("books").is_none());
        let error = command(&["books".to_owned()]).unwrap_err();
        assert!(
            error.contains("rss-miniflux") && error.contains("readlater"),
            "{error}"
        );
    }

    #[test]
    fn every_account_the_runtime_binds_to_a_server_may_be_set_for_its_app() {
        for account in ACCOUNTS {
            if account.holding == Holding::Bound {
                assert!(
                    kobo_policy::credentials::servers::may_set(account.app, account.secret),
                    "{}",
                    account.app
                );
            }
            assert_eq!(
                for_secret(account.app, account.secret).map(|found| found.app),
                Some(account.app)
            );
        }
    }

    #[test]
    fn a_typed_address_becomes_the_server_the_app_would_have_saved() {
        let readeck = account("readeck");
        assert_eq!(
            readeck.server("read.example/").unwrap(),
            "https://read.example"
        );
        assert!(readeck.server("http://read.example").is_err());
        assert!(readeck.server("https://user@read.example").is_err());
        let library = account("library");
        assert_eq!(
            library.server("https://books.example").unwrap(),
            "https://books.example/opds"
        );
        assert_eq!(
            library
                .server("https://books.example:8443/calibre/opds")
                .unwrap(),
            "https://books.example:8443/calibre/opds"
        );
        assert_eq!(
            library.binding("https://books.example:8443/calibre/opds"),
            "https://books.example:8443"
        );
        assert_eq!(
            account("stacks").server("ignored").unwrap(),
            "https://api.zotero.org"
        );
    }

    #[test]
    fn each_app_gets_its_settings_in_the_format_it_writes_itself() {
        let state =
            |name: &str, server: &str, answer: &[u8]| account(name).state(server, answer).unwrap();
        assert_eq!(
            state("readeck", "https://r.example", b""),
            [("server", "https://r.example".to_owned())]
        );
        assert_eq!(
            state("library", "https://b.example/opds", b""),
            [("catalog", "https://b.example/opds|calibre".to_owned())]
        );
        assert_eq!(
            state("digest", "https://m.example", b""),
            [("config", "https://m.example\nminiflux".to_owned())]
        );
        assert_eq!(
            state("kitchencard", "https://k.example", b""),
            [("config", "https://k.example\nmealie".to_owned())]
        );
        assert_eq!(
            state("homepanel", "https://h.example", b""),
            [("base-url", "https://h.example".to_owned())]
        );
        assert_eq!(
            state("post", "https://p.example", b""),
            [("gateway", "https://p.example".to_owned())]
        );
        assert_eq!(
            state(
                "stacks",
                "https://api.zotero.org",
                br#"{"key":"x","userID":475425,"username":"a"}"#
            ),
            [("zotero-user-id", "475425".to_owned())]
        );
        assert!(account("stacks")
            .state("https://api.zotero.org", b"{}")
            .is_err());
    }

    #[test]
    fn a_refused_check_blames_the_token_and_an_unanswered_one_the_address() {
        let readeck = account("readeck");
        let rejected = refused(
            readeck,
            "https://r.example",
            &kobo_net::Refusal {
                status: 401,
                retry_after: None,
                excerpt: String::new(),
            },
        );
        assert!(rejected.contains("did not accept the token"), "{rejected}");
        assert!(rejected.contains("API tokens"), "{rejected}");
        assert!(!rejected.contains("address"), "{rejected}");
        let library = refused(
            account("library"),
            "https://b.example/opds",
            &kobo_net::Refusal {
                status: 401,
                retry_after: None,
                excerpt: String::new(),
            },
        );
        assert!(library.contains("username or password"), "{library}");
        let missing = refused(
            readeck,
            "https://r.example",
            &kobo_net::Refusal {
                status: 404,
                retry_after: None,
                excerpt: String::new(),
            },
        );
        assert!(missing.contains("Check the address"), "{missing}");
        let silent = failure(
            readeck,
            "https://r.example",
            kobo_protocol::TaskError::Unreachable,
        );
        assert!(
            silent.contains("--ca ROOT.pem") && silent.contains("kobo trust set"),
            "{silent}"
        );
        assert!(failure(
            readeck,
            "https://r.example",
            kobo_protocol::TaskError::Offline
        )
        .contains("not on a network"));
        for message in [rejected, missing, silent] {
            assert!(message.ends_with("Nothing was installed."), "{message}");
        }
    }

    #[test]
    fn options_name_one_reader_and_one_token_source() {
        let readeck = account("readeck");
        let parsed = |words: &[&str]| {
            parse(
                readeck,
                &words
                    .iter()
                    .map(|word| (*word).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert!(parsed(&[
            "--sim",
            "--server",
            "https://r.example",
            "--token-file",
            "t"
        ])
        .is_ok());
        assert!(parsed(&["--sim", "--sim"]).is_err());
        assert!(parsed(&["--sim", "--reader", "den"]).is_err());
        assert!(parsed(&["--stdin", "--token-file", "t"]).is_err());
        assert!(parsed(&["--device", "host;reboot"]).is_err());
        assert!(parsed(&["--token", "secret"]).is_err());
    }

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kobo-connect-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn the_simulator_receives_the_same_record_and_settings_the_reader_would() {
        let root = scratch("sim");
        let readeck = account("readeck");
        let state = readeck.state("https://r.example", b"").unwrap();
        deliver_sim(
            &root.join("cobalt-sim-secrets"),
            &root.join("cobalt-sim-state/readeck"),
            &readeck.install("https://r.example", "fixture-token"),
            &state,
        )
        .unwrap();
        let expected = root.join("expected");
        kobo_policy::credentials::servers::install(
            &expected,
            "readeck",
            "readeck",
            "https://r.example",
            "fixture-token",
        )
        .unwrap();
        assert_eq!(
            fs::read(root.join("cobalt-sim-secrets/apps/readeck/servers/readeck")).unwrap(),
            fs::read(expected.join("apps/readeck/servers/readeck")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(root.join("cobalt-sim-state/readeck/server")).unwrap(),
            "https://r.example"
        );
        let panel = account("homepanel");
        let state = panel.state("https://h.example", b"").unwrap();
        deliver_sim(
            &root.join("cobalt-sim-secrets"),
            &root.join("cobalt-sim-state/homepanel"),
            &panel.install("", "ha-token"),
            &state,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("cobalt-sim-secrets/homeassistant"))
                .unwrap()
                .trim(),
            "ha-token"
        );
        assert_eq!(
            fs::read_to_string(root.join("cobalt-sim-state/homepanel/base-url")).unwrap(),
            "https://h.example"
        );
        fs::remove_dir_all(root).unwrap();
    }

    // The account record is installed by the reader's /bin/sh script, so these
    // run it against a scratch tree with a real shell, which is Unix's.
    #[cfg(unix)]
    fn run(script: &str) -> std::process::Output {
        std::process::Command::new("sh")
            .args(["-c", script])
            .output()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_reader_gets_the_bound_record_and_the_setting_while_cobalt_runs() {
        let root = scratch("device");
        let library = account("library");
        let server = "https://books.example/opds";
        let state = library.state(server, b"").unwrap();
        let binding = library.binding(server);
        let script = device_script(
            root.to_str().unwrap(),
            &library.install(&binding, "reader:pass word"),
            &state,
        );
        let output = run(&script);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.ends_with(b"ACCOUNT_CONNECTED\n"));
        let expected = root.join("expected");
        kobo_policy::credentials::servers::install(
            &expected,
            "calibre-web",
            "calibre",
            "https://books.example",
            "reader:pass word",
        )
        .unwrap();
        assert_eq!(
            fs::read(root.join("secrets/apps/calibre-web/servers/calibre")).unwrap(),
            fs::read(expected.join("apps/calibre-web/servers/calibre")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(root.join("state/calibre-web/catalog")).unwrap(),
            "https://books.example/opds|calibre"
        );
        assert!(!root.join(".server-account-setup.lock").exists());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("pass word"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_owner_token_replaces_a_typed_copy_and_leaves_no_staging_behind() {
        let root = scratch("owner");
        fs::create_dir_all(root.join("secrets/apps/kitchencard")).unwrap();
        fs::write(
            root.join("secrets/apps/kitchencard/mealie"),
            "typed-on-reader",
        )
        .unwrap();
        let kitchen = account("kitchencard");
        let state = kitchen.state("https://k.example", b"").unwrap();
        let output = run(&device_script(
            root.to_str().unwrap(),
            &kitchen.install("", "new-token"),
            &state,
        ));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.join("secrets/mealie"))
                .unwrap()
                .trim(),
            "new-token"
        );
        assert!(!root.join("secrets/apps/kitchencard/mealie").exists());
        assert_eq!(
            fs::read_to_string(root.join("state/kitchencard/config")).unwrap(),
            "https://k.example\nmealie"
        );
        let leftovers: Vec<_> = fs::read_dir(root.join("state/kitchencard"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_secret_can_be_given_to_the_simulator_the_way_an_owner_gives_one_to_a_reader() {
        let words = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let file = std::env::temp_dir().join(format!("sim-secret-{}", std::process::id()));
        fs::write(&file, "lip_token").unwrap();
        for line in [
            format!("set lichess --from {} --sim", file.display()),
            "remove lichess --sim".to_owned(),
            "list --sim".to_owned(),
        ] {
            let (_, target) = super::super::parse_secret(&words(&line)).unwrap();
            assert_eq!(target, super::super::SecretTarget::Sim, "{line}");
            assert_eq!(
                target.local_secrets(),
                Some(kobo_sim::simulated_secrets_root())
            );
        }
        assert!(super::super::parse_secret(&words("list --sim --device reader.local")).is_err());
        fs::remove_file(file).unwrap();
    }
}
