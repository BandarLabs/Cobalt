//! `kobo ui`: the companion as a window, for owners who do not live in a
//! terminal.
//!
//! Everything the window does, a command already did. What was missing was
//! somewhere to do it without typing: see that the reader is plugged in,
//! read what an install would change, press a button, watch it happen. So
//! this adds no second way of doing anything. It serves one page from this
//! process to this computer, and the page asks this process to call the
//! functions and run the commands that were already here.
//!
//! The process serves a small JSON interface and two front ends use it. On
//! a Mac, when the Cobalt app is installed beside this command, the app is
//! the front end: a native program that asks this process for everything
//! and draws it with the system's own controls. Everywhere else, and on a
//! Mac without the app, the front end is a page this process also serves,
//! opened in the default browser. A cross-platform toolkit would have been
//! several hundred new dependencies under a strict audit and one more
//! program to sign on every system; a page and one small Mac app are not.
//!
//! The app and this command find each other from either end. Run from a
//! terminal, this command starts the app and hands it the session. Opened
//! from the Dock, the app starts this command with `--for-app` and is
//! handed the session the same way. In both, what is handed over is one
//! line on a pipe only the two of them hold, never an argument, which any
//! account on the computer could read from the process list.
//!
//! # What keeps other things out
//!
//! The server listens on `127.0.0.1` only, on a port the system picks, and
//! stops when the window is closed. That is not a boundary by itself.
//! Loopback is shared: every program run by every account on the computer
//! can connect to the port, and any web page open in any browser can aim a
//! request at it. On Windows in particular there is nothing narrower to
//! bind to, and nothing but what follows stands between this server and
//! another account on the same machine. So the session's secrets are the
//! whole boundary, there and everywhere:
//!
//! * A random token travels once in the address this command opens. It is
//!   traded for a cookie and is dead from then on, so reading it afterwards
//!   from a process list, a history file or a screen buys nothing.
//! * Every request must name this server by its exact address, which is
//!   what turns away a public page that has re-pointed its own hostname at
//!   this computer.
//! * Every request that changes anything must also come from this page and
//!   carry a second token only the page was given.
//! * The Mac app has a token of its own in place of the cookie, sent on
//!   every request, and a request that carries it together with a browser's
//!   `Origin` is refused, so the token is no use to a page that learnt it.
//!
//! The rules are in [`guard`], the order they are applied in is in
//! [`server`], and each has a test that sees it refuse.

mod api;
mod arrival;
mod guard;
mod http;
mod jobs;
mod page;
mod server;
mod shopfront;
mod surroundings;
#[cfg(test)]
mod testing;

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::console;

const HELP: &str = "usage: kobo ui [--browser] [--no-open] [--volume PATH]

Opens Cobalt in a window. From it you can install Cobalt on a reader over
USB, update or remove it, add and remove apps, set up Send to Kobo in your
browsers, and send files and web articles to the reader.

The window is a page this command serves to this computer and nothing else.
It stays open until you close the window or press Ctrl-C here.

  --browser           Open in your default browser, not the Cobalt window
  --no-open           Print the address and open nothing
  --volume PATH       The mounted reader, when it is not found by itself
  --release-dir PATH  Install this verified release, not the one kobo came with

Every task in the window is also a command: kobo setup, kobo apps, kobo clip
and kobo send.";

/// How the companion gets in front of the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Showing {
    /// The Cobalt app when it is installed, else the default browser.
    Best,
    /// The default browser, because it was asked for.
    Browser,
    /// Nothing is opened; the address is printed.
    Nothing,
    /// The Cobalt app is already running and started this command. It is
    /// told where the server is and nothing is opened.
    ForApp,
}

#[derive(Debug, Eq, PartialEq)]
struct Options {
    showing: Showing,
    volume: Option<PathBuf>,
    release_dir: Option<PathBuf>,
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        showing: Showing::Best,
        volume: None,
        release_dir: None,
    };
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--browser" => options.showing = Showing::Browser,
            "--no-open" => options.showing = Showing::Nothing,
            "--for-app" => options.showing = Showing::ForApp,
            "--volume" | "-v" => {
                let path = words
                    .next()
                    .ok_or_else(|| console::usage("--volume needs the path to a mounted reader"))?;
                options.volume = Some(PathBuf::from(path));
            }
            "--release-dir" => {
                let path = words.next().ok_or_else(|| {
                    console::usage("--release-dir needs a verified release directory")
                })?;
                options.release_dir = Some(PathBuf::from(path));
            }
            other => {
                return Err(console::usage(format!(
                    "kobo ui [--browser] [--no-open] [--volume PATH] [--release-dir PATH]\n\
                     unknown option '{other}'"
                )));
            }
        }
    }
    Ok(options)
}

/// `kobo ui`.
///
/// # Errors
///
/// When the line is not understood, or the server cannot start.
pub fn command(arguments: &[String]) -> Result<(), String> {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{HELP}");
        return Ok(());
    }
    // Not an option an owner types and not in the help: the server runs
    // this command again with it, as the job that looks for a reader.
    if arguments == ["--find-reader"] {
        return arrival::find();
    }
    let options = parse(arguments)?;
    let machine = api::Machine::here(options.volume, options.release_dir)?;
    let program = machine.program.clone();
    let scratch = machine.scratch.clone();
    let server = server::Server::bind(
        machine,
        server::Limits::default(),
        server::Patience::default(),
    )?;
    let session = &server.shared.session;
    let handed = handoff(session.port, session.app_token());

    if options.showing == Showing::ForApp {
        // The one line the app is waiting for, and nothing else on this
        // stream, ever: the app reads it as data.
        println!("{handed}");
        std::io::stdout()
            .flush()
            .map_err(|error| format!("could not tell the app where the server is: {error}"))?;
        server.shared.tie();
        server.serve(when_input_ends());
        let _ = std::fs::remove_dir_all(scratch);
        return Ok(());
    }

    let address = session
        .launch_address()
        .ok_or("the session's address could not be made")?;
    let mut window = None;
    match show(options.showing, &program, &address, &handed) {
        Shown::Window(child) => {
            window = Some(child);
            server.shared.tie();
            println!("Cobalt is open in its own window. Close the window to finish, or press Ctrl-C here.");
        }
        Shown::Browser => {
            println!(
                "Cobalt is open in your browser. Close its tab to finish, or press Ctrl-C here."
            );
        }
        Shown::Nothing(why) => {
            if let Some(why) = why {
                println!("{why}");
            }
            // Said plainly that it works once, because the natural thing to
            // do with a printed address is to open it again later.
            println!(
                "Open this address in a browser on this computer. It opens the page once:\n\n  \
                 {address}\n\nPress Ctrl-C here to stop."
            );
        }
    }

    let stopped = server.serve(|| {
        window
            .as_mut()
            .is_some_and(|child| !matches!(child.try_wait(), Ok(None)))
    });
    let _ = std::fs::remove_dir_all(scratch);
    println!(
        "{}",
        match stopped {
            server::Stopped::ShellClosed | server::Stopped::WindowClosed => {
                "The window was closed."
            }
            server::Stopped::Idle => {
                "Nothing has asked for the page in ten minutes, so it has been closed. Run \
                 kobo ui to open it again."
            }
        }
    );
    Ok(())
}

/// What the app is told: where the server is, the token that is its way
/// in, and which release this command is, as one line of JSON.
///
/// The release is there so the app can notice it was installed with a
/// different one and say so, before it has asked the server anything.
fn handoff(port: u16, token: &str) -> String {
    serde_json::json!({
        "address": format!("http://127.0.0.1:{port}"),
        "token": token,
        "kobo": env!("CARGO_PKG_VERSION"),
    })
    .to_string()
}

/// Says when this process's standard input has ended.
///
/// The app that starts this command keeps the other end of that pipe open
/// for as long as it lives and never writes to it. The pipe ending is
/// therefore the app ending, however it ended: quit, crashed or killed. A
/// process cannot be told that its parent died, but it is always told that
/// a pipe closed, which is why the app's lifetime is tied to one.
fn when_input_ends() -> impl FnMut() -> bool {
    let ended = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&ended);
    std::thread::spawn(move || {
        let mut unread = [0_u8; 64];
        let mut input = std::io::stdin();
        while matches!(input.read(&mut unread), Ok(read) if read > 0) {}
        seen.store(true, Ordering::SeqCst);
    });
    move || ended.load(Ordering::SeqCst)
}

/// What was put in front of the owner.
enum Shown {
    /// The Cobalt app, as a child of this process.
    Window(Child),
    Browser,
    /// Nothing, by request or because nothing could be opened, with the
    /// reason when there is one worth saying.
    Nothing(Option<String>),
}

fn show(showing: Showing, program: &Path, address: &str, handed: &str) -> Shown {
    if matches!(showing, Showing::Nothing | Showing::ForApp) {
        return Shown::Nothing(None);
    }
    if showing == Showing::Best {
        if let Some(child) = window_beside(program).and_then(|window| open_window(&window, handed))
        {
            return Shown::Window(child);
        }
    }
    match open_in_browser(address) {
        Ok(()) => Shown::Browser,
        Err(why) => Shown::Nothing(Some(format!("No browser could be opened ({why})."))),
    }
}

/// The Cobalt window's program, when it is installed beside this one.
///
/// Only on a Mac, and only from the folder this command itself is in, which
/// is where the installer puts it. It is never looked for in `/Applications`
/// or on the path: the window is handed the launch address, and a program of
/// the same name from somewhere else is not something to hand it to.
fn window_beside(program: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let window = program
        .parent()?
        .join("Cobalt.app")
        .join("Contents")
        .join("MacOS")
        .join("Cobalt");
    window.is_file().then_some(window)
}

/// Starts the Cobalt app as a child of this process and hands it the
/// session.
///
/// Run directly, not through the system's `open`, for three reasons. A
/// child can be given a pipe, which is the only way to hand over a secret
/// that no other process can read. A child can be waited on, so the moment
/// the app quits is known exactly and the server stops with it. And a child
/// is in this terminal's foreground group, so Ctrl-C closes the app as well
/// as the server, where an app started through `open` would be left
/// showing a server that had gone.
///
/// The app is told to read its standard input by the environment, and the
/// line is written there and the pipe closed. If the app does not take it,
/// there is no app, and the caller falls back to the browser.
fn open_window(window: &Path, handed: &str) -> Option<Child> {
    let mut child = Command::new(window)
        .env("COBALT_UI_HANDOFF", "stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let delivered = child
        .stdin
        .take()
        .is_some_and(|mut pipe| writeln!(pipe, "{handed}").is_ok());
    if delivered {
        Some(child)
    } else {
        let _ = child.kill();
        let _ = child.wait();
        None
    }
}

/// Hands the address to the default browser.
///
/// Each system's own opener, given the address as one argument and never
/// through a shell, so nothing in it is interpreted on the way. On Windows
/// that rules out `start`, which is a shell built-in and would have its
/// argument parsed by `cmd`.
fn open_in_browser(address: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    let status = command
        .arg(address)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the opener exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &[&str]) -> Vec<String> {
        line.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn a_bare_ui_shows_the_best_window_there_is_and_the_flags_choose_otherwise() {
        assert_eq!(parse(&[]).expect("bare").showing, Showing::Best);
        assert_eq!(
            parse(&words(&["--browser"])).expect("browser").showing,
            Showing::Browser
        );
        assert_eq!(
            parse(&words(&["--no-open"])).expect("no-open").showing,
            Showing::Nothing
        );
        let named = parse(&words(&[
            "--volume",
            "/Volumes/KOBOeReader",
            "--release-dir",
            "/tmp/release",
        ]))
        .expect("paths");
        assert_eq!(
            named.volume.as_deref(),
            Some(Path::new("/Volumes/KOBOeReader"))
        );
        assert_eq!(
            named.release_dir.as_deref(),
            Some(Path::new("/tmp/release"))
        );
    }

    #[test]
    fn an_option_it_does_not_know_or_a_path_left_off_is_a_usage_error() {
        for line in [
            vec!["--port", "8080"],
            vec!["--volume"],
            vec!["--release-dir"],
            vec!["extra"],
        ] {
            let error = parse(&words(&line)).expect_err("a line that means nothing");
            assert_eq!(
                console::category_of(&error),
                console::EXIT_USAGE,
                "{line:?}"
            );
        }
    }

    #[test]
    fn nothing_is_opened_when_the_owner_asked_for_the_address_or_the_app_is_already_there() {
        for showing in [Showing::Nothing, Showing::ForApp] {
            assert!(matches!(
                show(
                    showing,
                    Path::new("/nowhere/kobo"),
                    "http://127.0.0.1:1/?token=x",
                    "{}"
                ),
                Shown::Nothing(None)
            ));
        }
        assert_eq!(
            parse(&words(&["--for-app"])).expect("for the app").showing,
            Showing::ForApp
        );
    }

    #[test]
    fn the_app_is_handed_one_line_naming_the_server_its_token_and_this_release() {
        let line = handoff(53124, "abc123");
        assert!(!line.contains('\n'));
        let told: serde_json::Value = serde_json::from_str(&line).expect("one JSON object");
        assert_eq!(told["address"], "http://127.0.0.1:53124");
        assert_eq!(told["token"], "abc123");
        assert_eq!(told["kobo"], env!("CARGO_PKG_VERSION"));
    }

    #[cfg(unix)]
    #[test]
    fn the_session_reaches_the_app_on_its_input_and_never_on_its_command_line() {
        use std::os::unix::fs::PermissionsExt as _;
        let folder = std::env::temp_dir().join(format!("kobo-ui-handoff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("folder");
        // A stand-in for the app: it records how it was started and what it
        // was sent, the way the app reads them.
        let app = folder.join("app");
        std::fs::write(
            &app,
            "#!/bin/sh\necho \"args:$*\" > \"$0.saw\"\necho \"told:$COBALT_UI_HANDOFF\" >> \"$0.saw\"\n\
             cat >> \"$0.saw\"\n",
        )
        .expect("stand-in");
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).expect("executable");
        let mut child = open_window(&app, "{\"token\":\"secret\"}").expect("started");
        child.wait().expect("the stand-in ends when its input does");
        let saw = std::fs::read_to_string(folder.join("app.saw")).expect("what it saw");
        assert_eq!(saw, "args:\ntold:stdin\n{\"token\":\"secret\"}\n");
        assert!(open_window(&folder.join("no-such-app"), "{}").is_none());
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn the_window_is_only_ever_the_one_installed_beside_this_command() {
        let folder = std::env::temp_dir().join(format!("kobo-ui-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("folder");
        let program = folder.join("kobo");
        assert_eq!(window_beside(&program), None);
        let inside = folder.join("Cobalt.app/Contents/MacOS");
        std::fs::create_dir_all(&inside).expect("bundle");
        std::fs::write(inside.join("Cobalt"), b"").expect("window");
        let found = window_beside(&program);
        if cfg!(target_os = "macos") {
            assert_eq!(found, Some(inside.join("Cobalt")));
        } else {
            assert_eq!(found, None);
        }
        let _ = std::fs::remove_dir_all(folder);
    }
}
