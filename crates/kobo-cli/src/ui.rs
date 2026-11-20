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
//! A page in a browser rather than a native toolkit, because the toolkit
//! would be several hundred new dependencies under a strict audit and a
//! second program to sign on every system, and the page is neither. On a
//! Mac the page is shown in a small window of its own when that window is
//! installed beside this command; everywhere else, and on a Mac without it,
//! it opens in the default browser.
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
//!
//! The rules are in [`guard`], the order they are applied in is in
//! [`server`], and each has a test that sees it refuse.

mod api;
mod guard;
mod http;
mod jobs;
mod page;
mod server;
#[cfg(test)]
mod testing;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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

/// How the page gets in front of the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Showing {
    /// The Cobalt window when it is installed, else the default browser.
    Best,
    /// The default browser, because it was asked for.
    Browser,
    /// Nothing is opened; the address is printed.
    Nothing,
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
    let options = parse(arguments)?;
    let machine = api::Machine::here(options.volume, options.release_dir)?;
    let program = machine.program.clone();
    let scratch = machine.scratch.clone();
    let server = server::Server::bind(
        machine,
        server::Limits::default(),
        server::Patience::default(),
    )?;
    let address = server
        .shared
        .session
        .launch_address()
        .ok_or("the session's address could not be made")?;

    let mut window = None;
    match show(options.showing, &program, &address) {
        Shown::Window(child) => {
            window = Some(child);
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

/// What was put in front of the owner.
enum Shown {
    /// The Cobalt window, as a child of this process.
    Window(Child),
    Browser,
    /// Nothing, by request or because nothing could be opened, with the
    /// reason when there is one worth saying.
    Nothing(Option<String>),
}

fn show(showing: Showing, program: &Path, address: &str) -> Shown {
    if showing == Showing::Nothing {
        return Shown::Nothing(None);
    }
    if showing == Showing::Best {
        if let Some(child) = window_beside(program).and_then(|window| open_window(&window, address))
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

/// Starts the Cobalt window as a child of this process.
///
/// Run directly, not through the system's `open`, for two reasons. A child
/// can be waited on, so the moment the window closes is known exactly and
/// the server stops with it. And a child is in this terminal's foreground
/// group, so Ctrl-C closes the window as well as the server, where a
/// window started through `open` would be left showing a page whose server
/// had gone.
fn open_window(window: &Path, address: &str) -> Option<Child> {
    Command::new(window)
        .arg(address)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
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
    fn nothing_is_opened_when_the_owner_asked_for_the_address() {
        assert!(matches!(
            show(
                Showing::Nothing,
                Path::new("/nowhere/kobo"),
                "http://127.0.0.1:1/?token=x"
            ),
            Shown::Nothing(None)
        ));
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
