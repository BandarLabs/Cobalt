//! The agents' side of the bargain: hook stdin in, decision stdout out.
//!
//! Both agents run a registered command when they want permission, hand it
//! the request as JSON on stdin, and read a decision from stdout. The same
//! binary serves both -- `kobo-sidekickd hook codex` and `hook claude` --
//! because the difference is only the JSON dialect at each end. In the
//! middle, both become one `POST /ask` to the daemon on loopback, blocking
//! until the person across the room decides.
//!
//! The cardinal rule is on the failure path: if the daemon is not running,
//! not reachable, or answers "pass", the hook prints nothing and exits
//! cleanly. To the agent that is a hook with no opinion, so the question
//! falls through to the terminal prompt the user already knows. Installing
//! these hooks can therefore never make anything worse.

use crate::http::post_local;
use crate::state;
use std::io::Read;
use std::time::Duration;

/// Longer than the daemon holds a question, so the daemon always answers
/// before this client would hang up on it.
const HOOK_PATIENCE: Duration = Duration::from_secs(330);

/// Reads one hook event from stdin, asks the daemon, prints the decision.
///
/// # Errors
///
/// Only for an agent this binary does not know. Everything downstream --
/// unreadable stdin, absent daemon -- deliberately succeeds in silence.
pub fn run_hook(agent: &str) -> Result<(), String> {
    if agent != "codex" && agent != "claude" {
        return Err(format!("unknown agent '{agent}'; expected codex or claude"));
    }
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return Ok(());
    }
    let ask = ask_from_event(agent, &input);
    let Ok(response) = post_local(state::HOOK_PORT, "/ask", &ask, HOOK_PATIENCE) else {
        return Ok(());
    };
    let decision = kobo_json::parse(&response)
        .ok()
        .and_then(|reply| {
            reply
                .get("decision")
                .and_then(kobo_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "pass".to_owned());
    if let Some(output) = decision_json(agent, &decision) {
        println!("{output}");
    }
    Ok(())
}

/// Normalises either agent's event into the daemon's `/ask` body.
///
/// Codex and Claude Code both send `tool_name` and a `tool_input` object;
/// for a shell command the interesting part is `tool_input.command`, and for
/// anything else the whole input serialised again is the honest summary.
#[must_use]
pub fn ask_from_event(agent: &str, event: &str) -> String {
    let parsed = kobo_json::parse(event).ok();
    let tool = parsed
        .as_ref()
        .and_then(|event| event.get("tool_name"))
        .and_then(kobo_json::Value::as_str)
        .unwrap_or("tool")
        .to_owned();
    let detail = parsed
        .as_ref()
        .and_then(|event| event.get("tool_input"))
        .map_or_else(String::new, |input| {
            input
                .get("command")
                .and_then(kobo_json::Value::as_str)
                .map_or_else(|| input.to_json(), str::to_owned)
        });
    kobo_json::ObjectBuilder::new()
        .set("source", agent)
        .set("tool", tool)
        .set("detail", detail)
        .build()
        .to_json()
}

/// The decision in the dialect the asking agent expects, or `None` for
/// "print nothing and let the terminal prompt have it".
#[must_use]
pub fn decision_json(agent: &str, decision: &str) -> Option<String> {
    if decision != "allow" && decision != "deny" {
        return None;
    }
    let output = if agent == "codex" {
        let verdict = kobo_json::ObjectBuilder::new()
            .set("behavior", decision)
            .set("message", "Decided on the Kobo")
            .build();
        kobo_json::ObjectBuilder::new().set(
            "hookSpecificOutput",
            kobo_json::ObjectBuilder::new()
                .set("hookEventName", "PermissionRequest")
                .set("decision", verdict)
                .build(),
        )
    } else {
        kobo_json::ObjectBuilder::new().set(
            "hookSpecificOutput",
            kobo_json::ObjectBuilder::new()
                .set("hookEventName", "PreToolUse")
                .set("permissionDecision", decision)
                .set("permissionDecisionReason", "Decided on the Kobo")
                .build(),
        )
    };
    Some(output.build().to_json())
}

/// Prints the configuration that registers the hook, ready to paste.
///
/// # Errors
///
/// Only for an agent this binary does not know.
pub fn setup(agent: &str) -> Result<(), String> {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_owned))
        .unwrap_or_else(|| "kobo-sidekickd".to_owned());
    match agent {
        "codex" => {
            println!("Add to ~/.codex/hooks.json (create it if absent):\n");
            println!("{{");
            println!("  \"hooks\": {{");
            println!("    \"PermissionRequest\": [");
            println!("      {{ \"hooks\": [ {{ \"type\": \"command\",");
            println!("          \"command\": [\"{binary}\", \"hook\", \"codex\"],");
            println!("          \"timeout\": 360 }} ] }}");
            println!("    ]");
            println!("  }}");
            println!("}}");
            println!("\nWorks for Codex CLI and the Codex desktop app alike:");
            println!("both run the same core, which reads the same hooks.");
        }
        "claude" => {
            println!("Add to ~/.claude/settings.json under \"hooks\":\n");
            println!("{{");
            println!("  \"hooks\": {{");
            println!("    \"PreToolUse\": [");
            println!("      {{ \"matcher\": \"*\",");
            println!("        \"hooks\": [ {{ \"type\": \"command\",");
            println!("          \"command\": \"{binary} hook claude\",");
            println!("          \"timeout\": 360 }} ] }}");
            println!("    ]");
            println!("  }}");
            println!("}}");
            println!("\nClaude Code shows its usual prompt whenever the hook");
            println!("declines to decide, so nothing breaks without the daemon.");
        }
        _ => return Err(format!("unknown agent '{agent}'; expected codex or claude")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ask_from_event, decision_json};

    #[test]
    fn a_codex_shell_request_becomes_an_ask_with_the_command_line() {
        let event = r#"{"tool_name":"shell","tool_input":{"command":"rm -rf ./build"}}"#;
        let ask = kobo_json::parse(&ask_from_event("codex", event)).expect("valid json");
        assert_eq!(
            ask.get("source").and_then(kobo_json::Value::as_str),
            Some("codex")
        );
        assert_eq!(
            ask.get("tool").and_then(kobo_json::Value::as_str),
            Some("shell")
        );
        assert_eq!(
            ask.get("detail").and_then(kobo_json::Value::as_str),
            Some("rm -rf ./build")
        );
    }

    #[test]
    fn a_tool_without_a_command_shows_its_whole_input_instead() {
        let event = r#"{"tool_name":"Write","tool_input":{"file_path":"/etc/hosts"}}"#;
        let ask = kobo_json::parse(&ask_from_event("claude", event)).expect("valid json");
        let detail = ask
            .get("detail")
            .and_then(kobo_json::Value::as_str)
            .expect("detail");
        assert!(detail.contains("/etc/hosts"), "{detail}");
    }

    #[test]
    fn garbage_on_stdin_still_produces_a_well_formed_ask() {
        let ask = kobo_json::parse(&ask_from_event("codex", "not json")).expect("valid json");
        assert_eq!(
            ask.get("source").and_then(kobo_json::Value::as_str),
            Some("codex")
        );
        assert_eq!(
            ask.get("tool").and_then(kobo_json::Value::as_str),
            Some("tool")
        );
    }

    #[test]
    fn an_allow_speaks_each_agents_dialect() {
        let codex = decision_json("codex", "allow").expect("a decision");
        assert!(
            codex.contains("\"hookEventName\":\"PermissionRequest\""),
            "{codex}"
        );
        assert!(codex.contains("\"behavior\":\"allow\""), "{codex}");
        let claude = decision_json("claude", "allow").expect("a decision");
        assert!(
            claude.contains("\"hookEventName\":\"PreToolUse\""),
            "{claude}"
        );
        assert!(
            claude.contains("\"permissionDecision\":\"allow\""),
            "{claude}"
        );
    }

    #[test]
    fn a_pass_prints_nothing_so_the_terminal_prompt_takes_over() {
        assert_eq!(decision_json("codex", "pass"), None);
        assert_eq!(decision_json("claude", "pass"), None);
        assert_eq!(decision_json("codex", "gibberish"), None);
    }
}
