//! Computer-side setup for runtime-owned, server-bound accounts.
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str =
    "kobo secret set NAME --app APP --server HTTPS_URL (--from PATH | --stdin) --device HOST";
struct Options {
    name: String,
    app: String,
    server: String,
    host: String,
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
    let script = install_script("/mnt/onboard/.adds/cobalt", &options, &token);
    // Remote shell diagnostics can echo script input. Never display them.
    let output = super::run_remote_shell(
        &format!("root@{}", options.host),
        &script,
        Duration::from_secs(60),
    )
    .map_err(|_| "Account setup could not complete over SSH; no credential output is displayed.")?;
    if !output.status.success() || output.stdout != b"SERVER_ACCOUNT_SAVED\n" {
        return Err(match output.status.code() {
            Some(20) => "Exit Cobalt normally and wait for recovery to finish, then retry. Do not kill the daemon.",
            Some(21) => "Unsafe credential path or occupied setup lock. Inspect the reader before retrying.",
            _ => "Account setup was not confirmed. Keep Cobalt closed and check the reader before retrying.",
        }.into());
    }
    println!("Server-bound account installed. Configure the same server in the app; see its setup guide. The token was not printed or checked against the server.");
    Ok(())
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    if arguments.first().map(String::as_str) != Some("set") {
        return Err(USAGE.into());
    }
    let name = arguments.get(1).ok_or(USAGE)?.clone();
    let (mut app, mut server, mut host, mut source) = (None, None, None, None);
    let mut stdin = false;
    let mut args = arguments[2..].iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--app" if app.is_none() => app = Some(args.next().ok_or(USAGE)?.clone()),
            "--server" if server.is_none() => {
                server = Some(args.next().ok_or(USAGE)?.trim_end_matches('/').to_owned());
            }
            "--device" if host.is_none() => host = Some(args.next().ok_or(USAGE)?.clone()),
            "--from" if source.is_none() && !stdin => {
                source = Some(PathBuf::from(args.next().ok_or(USAGE)?));
            }
            "--stdin" if source.is_none() && !stdin => stdin = true,
            _ => return Err(USAGE.into()),
        }
    }
    let app = app.ok_or(USAGE)?;
    let server = server.ok_or(USAGE)?;
    let host = host.ok_or(USAGE)?;
    if !kobo_policy::credentials::servers::may_set(&app, &name) {
        return Err("This app/account pair is not approved for server-bound credentials.".into());
    }
    if !kobo_policy::credentials::servers::valid_server(&server) || server.contains('%') {
        return Err(
            "Use an HTTPS server without credentials, a query, fragment or encoded path.".into(),
        );
    }
    if !super::valid_device_host(&host) || (source.is_none() && !stdin) {
        return Err(USAGE.into());
    }
    Ok(Options {
        name,
        app,
        server,
        host,
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
            "Use 1–{limit} printable ASCII bytes without spaces or embedded newlines."
        ));
    }
    String::from_utf8(bytes).map_err(|_| "Invalid token encoding.".into())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn install_script(root: &str, options: &Options, token: &str) -> String {
    let record = super::base64_encode(
        format!("cobalt-server-account-v1\n{}\n{token}", options.server).as_bytes(),
    );
    format!(
        r#"set -eu
set +x
umask 077
root={root}
app={app}
name={name}
command -v pidof >/dev/null 2>&1 || exit 22
idle() {{
  if pidof kobod >/dev/null 2>&1 || pidof recovery-kobod >/dev/null 2>&1 || pidof "kobo-$app" >/dev/null 2>&1; then exit 20; fi
}}
idle
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
account="$root/secrets/apps/$app/servers/$name"
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
idle
mv -f "$lock/new-account" "$account"
sync
printf 'SERVER_ACCOUNT_SAVED\n'
"#,
        root = quote(root),
        app = quote(&options.app),
        name = quote(&options.name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
    fn shell_record_matches_runtime_and_refuses_busy_or_unsafe_paths() {
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
        fs::write(&pidof, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&pidof, fs::Permissions::from_mode(0o700)).unwrap();
        // Resolve macOS /var symlinks: production rejects symlinked ancestors.
        let root = root.canonicalize().unwrap();
        let options = parse(&args()).unwrap();
        let run = || {
            let mut child = Command::new("sh")
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    install_script(root.to_str().unwrap(), &options, "fixture-token").as_bytes(),
                )
                .unwrap();
            child.wait_with_output().unwrap()
        };
        assert!(run().status.success());
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
        let previous = b"previous-account";
        fs::write(&account, previous).unwrap();
        let mv = bin.join("mv");
        fs::write(&mv, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&mv, fs::Permissions::from_mode(0o700)).unwrap();
        let failed = run();
        assert!(!failed.status.success());
        assert!(failed.stdout.is_empty());
        assert_eq!(fs::read(&account).unwrap(), previous);
        assert!(!root.join(".server-account-setup.lock").exists());
        fs::remove_file(mv).unwrap();
        fs::write(&pidof, "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(run().status.code(), Some(20));
        fs::write(&pidof, "#!/bin/sh\nexit 1\n").unwrap();
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
