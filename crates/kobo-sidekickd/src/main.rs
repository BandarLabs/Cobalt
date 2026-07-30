//! The sidekick daemon: a coding agent's permission prompt, on a Kobo.
//!
//! Coding agents stop mid-task to ask "may I run this?", and the asking is
//! wherever the terminal is. This daemon catches those questions through the
//! agents' own hook systems -- Claude Code's `PreToolUse`, Codex's
//! `PermissionRequest` -- and holds them for a Kobo across the room, where
//! the sidekick application shows the command and three buttons. The answer
//! travels back and the hook returns it as if the person had been at the
//! keyboard.
//!
//! Nothing about the agents' setup changes. The hooks are registered once in
//! their configuration files, fire no matter which frontend asked, and when
//! this daemon is unreachable they decline to decide, so the question falls
//! through to the terminal prompt it always was.
//!
//! Two listeners, deliberately different: hooks talk plaintext on loopback,
//! because they are on the same machine; the reader talks TLS on the LAN,
//! verified against the self-signed root that `init` generates and the owner
//! installs with `kobo trust set`. A pairing code rides along as a token so
//! a neighbour on the network cannot answer for you.

mod board;
mod hooks;
mod http;
mod server;
mod state;

use std::process::ExitCode;

const USAGE: &str = "usage: kobo-sidekickd init [--host ADDRESS ...]\n\
                     \x20      kobo-sidekickd run\n\
                     \x20      kobo-sidekickd hook (codex | claude)\n\
                     \x20      kobo-sidekickd setup (codex | claude)";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.split_first() {
        Some((verb, rest)) => match (verb.as_str(), rest) {
            ("init", extra) => state::init(extra),
            ("run", []) => server::run(),
            ("hook", [agent]) => hooks::run_hook(agent),
            ("setup", [agent]) => hooks::setup(agent),
            _ => Err(USAGE.to_owned()),
        },
        None => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
