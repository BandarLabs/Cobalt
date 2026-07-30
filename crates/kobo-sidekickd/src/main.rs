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

mod agents;
mod board;
mod hooks;
mod http;
mod server;
mod state;

use std::process::ExitCode;

const USAGE: &str = "usage: kobo-sidekickd init [--host ADDRESS ...]\n\
                     \x20      kobo-sidekickd run\n\
                     \x20      kobo-sidekickd setup [AGENT] [--dry-run]\n\
                     \x20      kobo-sidekickd setup AGENT --print\n\
                     \x20      kobo-sidekickd agents\n\
                     \x20      kobo-sidekickd hook AGENT\n\
                     \n\
                     setup with no agent registers every one it finds.";

/// `setup`, with an optional agent and an optional `--dry-run`, in either
/// order, because nobody should have to remember which comes first.
fn parse_setup(arguments: &[String]) -> Result<(), String> {
    let mut agent = None;
    let mut dry_run = false;
    for argument in arguments {
        match argument.as_str() {
            "--dry-run" => dry_run = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option '{other}'\n{USAGE}"))
            }
            other if agent.is_none() => agent = Some(other),
            other => return Err(format!("unexpected '{other}'\n{USAGE}")),
        }
    }
    hooks::setup(agent, dry_run)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.split_first() {
        Some((verb, rest)) => match (verb.as_str(), rest) {
            ("init", extra) => state::init(extra),
            ("run", []) => server::run(),
            ("hook", [agent]) => hooks::run_hook(agent),
            ("agents", []) => hooks::list(),
            ("setup", [agent, flag]) if flag == "--print" => hooks::print_setup(agent),
            ("setup", extra) => parse_setup(extra),
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
