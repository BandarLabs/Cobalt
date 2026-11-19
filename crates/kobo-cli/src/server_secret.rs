//! Computer-side setup for runtime-owned, server-bound accounts.
//!
//! The same delivery `kobo connect` uses, for an owner who already has the
//! token in a file and knows the secret's name. The token is checked against
//! the server first when this command knows the service, and the app's own
//! server setting is written beside it, so the address is typed once, here.
use super::account::{self, Destination, Holding, Install};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

const USAGE: &str =
    "kobo secret set NAME --app APP --server HTTPS_URL (--from PATH | --stdin) (--device HOST | --sim)";
struct Options {
    name: String,
    app: String,
    server: String,
    destination: Destination,
    source: Option<PathBuf>,
}

pub fn requested(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|arg| matches!(arg.as_str(), "--app" | "--server" | "--stdin"))
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    let options = parse(arguments)?;
    let token = if let Some(path) = &options.source {
        let metadata = fs::symlink_metadata(path).map_err(|_| "Cannot read token file.")?;
        if !metadata.is_file() {
            return Err("Use a regular token file, not a symlink.".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("Run chmod 600 on the token file, or use --stdin.".into());
            }
        }
        read_value(fs::File::open(path).map_err(|_| "Cannot open token file.")?)?
    } else {
        read_value(std::io::stdin().lock())?
    };
    if options.destination == Destination::Sim {
        account::prepare_sim()?;
    }
    let known = account::for_secret(&options.app, &options.name);
    let (server, state) = match known {
        Some(service) => {
            let server = service.server(&options.server)?;
            let answer = account::check(service, &server, &token)?;
            let state = service.state(&server, &answer)?;
            (server, state)
        }
        None => (options.server.clone(), Vec::new()),
    };
    let binding = known.map_or_else(|| server.clone(), |service| service.binding(&server));
    let install = Install {
        app: &options.app,
        secret: &options.name,
        holding: Holding::Bound,
        server: &binding,
        value: &token,
    };
    account::deliver(&install, &options.destination, &state)?;
    match known {
        Some(service) => println!(
            "Checked the token with {} and installed it for {} at {server}. {} has its server address too: open it, or tap Check again if it is already open.",
            service.service, service.title, service.title
        ),
        None => println!(
            "Installed the account for {} at {server}. This command cannot check this service, so the token was not checked. Set the same server in the app.",
            options.app
        ),
    }
    Ok(())
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    if arguments.first().map(String::as_str) != Some("set") {
        return Err(USAGE.into());
    }
    let name = arguments.get(1).ok_or(USAGE)?.clone();
    let (mut app, mut server, mut destination, mut source) = (None, None, None, None);
    let mut stdin = false;
    let mut args = arguments[2..].iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--app" if app.is_none() => app = Some(args.next().ok_or(USAGE)?.clone()),
            "--server" if server.is_none() => {
                server = Some(args.next().ok_or(USAGE)?.trim_end_matches('/').to_owned());
            }
            "--device" if destination.is_none() => {
                let host = args.next().ok_or(USAGE)?.clone();
                if !super::valid_device_host(&host) {
                    return Err(USAGE.into());
                }
                destination = Some(Destination::Device(host));
            }
            "--sim" if destination.is_none() => destination = Some(Destination::Sim),
            "--from" if source.is_none() && !stdin => {
                source = Some(PathBuf::from(args.next().ok_or(USAGE)?));
            }
            "--stdin" if source.is_none() && !stdin => stdin = true,
            _ => return Err(USAGE.into()),
        }
    }
    let app = app.ok_or(USAGE)?;
    let server = server.ok_or(USAGE)?;
    let destination = destination.ok_or(USAGE)?;
    if !kobo_policy::credentials::servers::may_set(&app, &name) {
        return Err("This app/account pair is not approved for server-bound credentials.".into());
    }
    if !kobo_policy::credentials::servers::valid_server(&server) || server.contains('%') {
        return Err(
            "Use an HTTPS server without credentials, a query, fragment or encoded path.".into(),
        );
    }
    if source.is_none() && !stdin {
        return Err(USAGE.into());
    }
    Ok(Options {
        name,
        app,
        server,
        destination,
        source,
    })
}

fn read_value(reader: impl Read) -> Result<String, String> {
    let limit = kobo_protocol::MAX_APP_SECRET_BYTES;
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 3)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read token.")?;
    if bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    if bytes.is_empty() || bytes.len() > limit || !bytes.iter().all(u8::is_ascii_graphic) {
        return Err(format!(
            "Use 1 to {limit} printable ASCII bytes without spaces or embedded newlines."
        ));
    }
    String::from_utf8(bytes).map_err(|_| "Invalid token encoding.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    fn args() -> Vec<String> {
        [
            "set",
            "readeck",
            "--app",
            "readeck",
            "--server",
            "https://read.example/library/",
            "--stdin",
            "--device",
            "reader.local",
        ]
        .map(str::to_owned)
        .to_vec()
    }
    #[test]
    fn approved_account_requires_one_private_source_and_safe_destinations() {
        assert_eq!(
            parse(&args()).unwrap().server,
            "https://read.example/library"
        );
        for (index, value) in [
            (0, "remove"),
            (1, "other"),
            (3, "other"),
            (5, "http://read.example"),
            (5, "https://user@read.example"),
            (5, "https://read.example/../x"),
            (5, "https://read.example/%2e"),
            (8, "host;reboot"),
        ] {
            let mut input = args();
            input[index] = value.into();
            assert!(parse(&input).is_err());
        }
        for extra in [
            vec!["--stdin"],
            vec!["--from", "file"],
            vec!["--token", "secret"],
        ] {
            let mut input = args();
            input.extend(extra.into_iter().map(str::to_owned));
            assert!(parse(&input).is_err());
        }
        let mut input = args();
        input.remove(6);
        assert!(parse(&input).is_err());
    }
    #[test]
    fn bounded_input_accepts_clipboard_newline_only() {
        for value in [&b"abc"[..], &b"abc\n"[..], &b"abc\r\n"[..]] {
            assert_eq!(read_value(value).unwrap(), "abc");
        }
        for value in [
            &b""[..],
            &b" abc"[..],
            &b"a b"[..],
            &b"a\nb"[..],
            &b"a\n\n"[..],
            &b"\xff"[..],
        ] {
            assert!(read_value(value).is_err());
        }
        assert!(read_value(&vec![b'x'; kobo_protocol::MAX_APP_SECRET_BYTES + 1][..]).is_err());
    }
    #[test]
    fn a_simulator_destination_is_accepted_in_place_of_a_reader() {
        let mut input = args();
        input.truncate(7);
        input.push("--sim".into());
        assert_eq!(parse(&input).unwrap().destination, Destination::Sim);
        input.extend(["--device".to_owned(), "reader.local".to_owned()]);
        assert!(parse(&input).is_err());
    }

    // The record is installed by the reader's /bin/sh script, and the fixture
    // stubs pidof and mv as shell scripts and checks Unix mode bits.
    #[cfg(unix)]
    #[test]
    fn the_record_installs_while_cobalt_runs_but_waits_out_an_update() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir().join(format!(
            "server-secret-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let pidof = bin.join("pidof");
        // Cobalt and the app are running; no update is.
        let running = "#!/bin/sh\ncase \"$1\" in recovery-kobod) exit 1;; *) exit 0;; esac\n";
        fs::write(&pidof, running).unwrap();
        fs::set_permissions(&pidof, fs::Permissions::from_mode(0o700)).unwrap();
        // Resolve macOS /var symlinks: production rejects symlinked ancestors.
        let root = root.canonicalize().unwrap();
        let options = parse(&args()).unwrap();
        let install = Install {
            app: &options.app,
            secret: &options.name,
            holding: Holding::Bound,
            server: &options.server,
            value: "fixture-token",
        };
        let state = [("server", options.server.clone())];
        let run = || {
            Command::new("sh")
                .args([
                    "-c",
                    &account::device_script(root.to_str().unwrap(), &install, &state),
                ])
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                )
                .stdin(Stdio::null())
                .output()
                .unwrap()
        };
        let done = run();
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
        let account = root.join("secrets/apps/readeck/servers/readeck");
        let expected = root.join("expected");
        kobo_policy::credentials::servers::install(
            &expected,
            "readeck",
            "readeck",
            &options.server,
            "fixture-token",
        )
        .unwrap();
        assert_eq!(
            fs::read(&account).unwrap(),
            fs::read(expected.join("apps/readeck/servers/readeck")).unwrap()
        );
        assert_eq!(
            fs::metadata(&account).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_to_string(root.join("state/readeck/server")).unwrap(),
            "https://read.example/library"
        );
        let previous = b"previous-account";
        fs::write(&account, previous).unwrap();
        let mv = bin.join("mv");
        fs::write(&mv, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&mv, fs::Permissions::from_mode(0o700)).unwrap();
        let failed = run();
        assert!(!failed.status.success());
        assert!(!failed.stdout.ends_with(b"ACCOUNT_CONNECTED\n"));
        assert_eq!(fs::read(&account).unwrap(), previous);
        assert!(!root.join(".server-account-setup.lock").exists());
        fs::remove_file(mv).unwrap();
        fs::write(&pidof, "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(
            run().status.code(),
            Some(20),
            "an update in progress is waited out"
        );
        fs::write(&pidof, running).unwrap();
        fs::create_dir(root.join(".server-account-setup.lock")).unwrap();
        assert_eq!(run().status.code(), Some(21));
        fs::remove_dir(root.join(".server-account-setup.lock")).unwrap();
        fs::remove_file(&account).unwrap();
        symlink(root.join("untouched"), &account).unwrap();
        assert_eq!(run().status.code(), Some(21));
        assert!(!root.join("untouched").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
