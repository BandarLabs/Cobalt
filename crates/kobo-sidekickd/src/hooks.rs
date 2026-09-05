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

use crate::agents;
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
    let known = agents::find(agent)?;
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return Ok(());
    }
    let Ok(event) = kobo_json::parse(&input) else {
        return Ok(());
    };
    if tool_name(&event) == "AskUserQuestion" {
        answer_questions(known.id, &event);
    } else {
        decide_permission(known.id, &event);
    }
    Ok(())
}

/// The usual case: one thing the agent wants to do, and a yes or a no.
///
/// The "always allow" lines the terminal would have shown come across as
/// extra buttons, so the reader offers exactly what the keyboard would.
fn decide_permission(agent: &str, event: &kobo_json::Value) {
    let tool = tool_name(event);
    let detail = event.get("tool_input").map_or_else(String::new, describe);
    let offered = suggestions(event);
    let choices: Vec<kobo_json::Value> = offered
        .iter()
        .map(|(label, description, _)| {
            kobo_json::ObjectBuilder::new()
                .set("label", label.as_str())
                .set("description", description.as_str())
                .build()
        })
        .collect();
    let session = session_identity(event);
    let Some((decision, labels)) = ask_daemon(
        agent,
        &session,
        &tool,
        &detail,
        choices,
        Wants::permission(),
    ) else {
        return;
    };
    if decision == "chose" {
        // An "always allow" was pressed. Echoing the suggestion back is
        // documented as the same thing as picking it in the dialog.
        let pressed = labels.first().map_or("", String::as_str);
        if let Some((_, _, entry)) = offered.iter().find(|(name, _, _)| name == pressed) {
            println!("{}", always(entry.clone()));
        }
        return;
    }
    if let Some(output) = decision_json(agent, &decision) {
        println!("{output}");
    }
}

/// A multiple-choice question, put to the person one at a time.
///
/// The agent sends up to four questions in one call. The reader shows a
/// screen at a time and this loop holds them together, so nothing further
/// down needs a notion of "question three of four". If any question goes
/// unanswered the whole call is left for the terminal, because a half
/// answered set is worse than none.
fn answer_questions(agent: &str, event: &kobo_json::Value) {
    let Some(input) = event.get("tool_input") else {
        return;
    };
    let Some(questions) = input.get("questions").and_then(kobo_json::Value::as_array) else {
        return;
    };
    if questions.is_empty() {
        return;
    }
    let mut answers = Vec::new();
    for question in questions {
        let text = string(question, "question");
        let choices: Vec<kobo_json::Value> = question
            .get("options")
            .and_then(kobo_json::Value::as_array)
            .map(<[kobo_json::Value]>::to_vec)
            .unwrap_or_default();
        if choices.is_empty() {
            return;
        }
        // The header is a label of twelve characters at most, which is a
        // byline; the question itself is the thing to read.
        let header = string(question, "header");
        let tool = if header.is_empty() {
            "Question".to_owned()
        } else {
            header
        };
        // Claude Code documents an array for a multi-select answer and a
        // plain string for a single one, so each is sent as it is written.
        let multi = question
            .get("multiSelect")
            .and_then(kobo_json::Value::as_bool)
            == Some(true);
        let session = session_identity(event);
        let Some((decision, labels)) = ask_daemon(
            agent,
            &session,
            &tool,
            &text,
            choices,
            Wants::question(multi),
        ) else {
            return;
        };
        if decision != "chose" || labels.is_empty() {
            return;
        }
        let answer = if multi {
            kobo_json::Value::Array(labels.into_iter().map(kobo_json::Value::String).collect())
        } else {
            kobo_json::Value::String(labels.into_iter().next().unwrap_or_default())
        };
        answers.push((text, answer));
    }
    let updated = kobo_json::ObjectBuilder::new()
        // Passed through unchanged, which the tool requires.
        .set("questions", kobo_json::Value::Array(questions.to_vec()))
        .set("answers", kobo_json::Value::Object(answers))
        .build();
    println!("{}", answered(updated));
}

/// Puts one question to the daemon and waits. `None` when there is nobody
/// to ask or nothing was decided, which is the silence the agent reads as
/// "this hook has no opinion".
fn ask_daemon(
    agent: &str,
    session: &str,
    tool: &str,
    detail: &str,
    choices: Vec<kobo_json::Value>,
    wants: Wants,
) -> Option<(String, Vec<String>)> {
    let body = kobo_json::ObjectBuilder::new()
        .set("source", agent)
        .set("session", session)
        .set("tool", tool)
        .set("detail", detail)
        .set("choices", kobo_json::Value::Array(choices))
        .set("permission", wants.permission)
        .set("multi", wants.multi)
        .build()
        .to_json();
    let response = post_local(state::HOOK_PORT, "/ask", &body, HOOK_PATIENCE).ok()?;
    let reply = kobo_json::parse(&response).ok()?;
    let decision = string(&reply, "decision");
    if decision.is_empty() || decision == "pass" {
        return None;
    }

    let labels = reply
        .get("labels")
        .and_then(kobo_json::Value::as_array)
        .map(<[kobo_json::Value]>::to_vec)
        .unwrap_or_default()
        .iter()
        .filter_map(|label| label.as_str().map(str::to_owned))
        .collect();
    Some((decision, labels))
}

/// A concise stable identity for the terminal that sent a hook. Both fields
/// are optional in hook dialects, so absence is a blank label, never refusal.
fn session_identity(event: &kobo_json::Value) -> String {
    let cwd = string(event, "cwd");
    let session = string(event, "session_id");
    let project = cwd.rsplit('/').find(|part| !part.is_empty()).unwrap_or("");
    let suffix: String = session
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    match (project.is_empty(), suffix.is_empty()) {
        (true, true) => String::new(),
        (false, true) => project.to_owned(),
        (true, false) => suffix,
        (false, false) => format!("{project} · {suffix}"),
    }
}

/// What kind of answer a question will take.
#[derive(Clone, Copy)]
struct Wants {
    /// Whether allow and deny mean anything.
    permission: bool,
    /// Whether more than one choice may be taken.
    multi: bool,
}

impl Wants {
    const fn permission() -> Self {
        Self {
            permission: true,
            multi: false,
        }
    }

    const fn question(multi: bool) -> Self {
        Self {
            permission: false,
            multi,
        }
    }
}

/// The "always allow" lines this request came with, each as a button and
/// the entry to echo back if it is the one pressed.
fn suggestions(event: &kobo_json::Value) -> Vec<(String, String, kobo_json::Value)> {
    event
        .get("permission_suggestions")
        .and_then(kobo_json::Value::as_array)
        .map(<[kobo_json::Value]>::to_vec)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let (label, description) = describe_suggestion(&entry)?;
            Some((label, description, entry))
        })
        .collect()
}

/// What one "always allow" entry should say on a button.
///
/// These arrive as instructions rather than as words -- a rule to add, a
/// mode to set -- so the button has to be written here. Anything with no
/// sensible wording is dropped rather than drawn as a shrug.
fn describe_suggestion(entry: &kobo_json::Value) -> Option<(String, String)> {
    let scope = match string(entry, "destination").as_str() {
        "session" => "for the rest of this session",
        "localSettings" | "projectSettings" => "in this project",
        "userSettings" => "everywhere",
        _ => "",
    };
    let with_scope = |text: String| match (text.is_empty(), scope.is_empty()) {
        (true, _) => scope.to_owned(),
        (false, true) => text,
        (false, false) => format!("{text}, {scope}"),
    };
    match string(entry, "type").as_str() {
        "addRules" => {
            let verb = match string(entry, "behavior").as_str() {
                "deny" => "Always deny",
                "ask" => "Always ask about",
                _ => "Always allow",
            };
            let rules = entry
                .get("rules")
                .and_then(kobo_json::Value::as_array)
                .map(<[kobo_json::Value]>::to_vec)
                .unwrap_or_default();
            let named: Vec<String> = rules
                .iter()
                .map(|rule| {
                    let tool = string(rule, "toolName");
                    let content = string(rule, "ruleContent");
                    if content.is_empty() {
                        tool
                    } else {
                        format!("{tool} {content}")
                    }
                })
                .filter(|text| !text.is_empty())
                .collect();
            if named.is_empty() {
                return None;
            }
            Some((verb.to_owned(), with_scope(named.join(", "))))
        }
        "setMode" => {
            let mode = string(entry, "mode");
            let words = match mode.as_str() {
                "acceptEdits" => "Accept edits",
                "auto" => "Decide automatically",
                "dontAsk" => "Stop asking",
                "bypassPermissions" => "Skip every permission",
                "plan" => "Plan only",
                _ => return None,
            };
            Some((words.to_owned(), with_scope(String::new())))
        }
        "addDirectories" => {
            let directories = entry
                .get("directories")
                .and_then(kobo_json::Value::as_array)
                .map(<[kobo_json::Value]>::to_vec)
                .unwrap_or_default();
            let named: Vec<String> = directories
                .iter()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect();
            if named.is_empty() {
                return None;
            }
            Some(("Add directory".to_owned(), with_scope(named.join(", "))))
        }
        _ => None,
    }
}

/// The reply that grants this once and adopts the "always allow" pressed.
#[must_use]
fn always(entry: kobo_json::Value) -> String {
    let decision = kobo_json::ObjectBuilder::new()
        .set("behavior", "allow")
        .set("message", "Decided on the Kobo")
        .set("updatedPermissions", kobo_json::Value::Array(vec![entry]))
        .build();
    kobo_json::ObjectBuilder::new()
        .set(
            "hookSpecificOutput",
            kobo_json::ObjectBuilder::new()
                .set("hookEventName", "PermissionRequest")
                .set("decision", decision)
                .build(),
        )
        .build()
        .to_json()
}

/// The reply that carries the answers back as the tool's own result.
///
/// A multiple-choice question is not a permission to grant, so allowing it
/// on its own would only make the terminal ask again. The answers ride back
/// as the tool's input, which the tool reads as having been answered.
#[must_use]
fn answered(updated: kobo_json::Value) -> String {
    kobo_json::ObjectBuilder::new()
        .set(
            "hookSpecificOutput",
            kobo_json::ObjectBuilder::new()
                .set("hookEventName", "PreToolUse")
                .set("permissionDecision", "allow")
                .set("updatedInput", updated)
                .build(),
        )
        .build()
        .to_json()
}

/// One string field, or the empty string.
fn string(value: &kobo_json::Value, name: &str) -> String {
    value
        .get(name)
        .and_then(kobo_json::Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// Which tool the event is about.
fn tool_name(event: &kobo_json::Value) -> String {
    match string(event, "tool_name") {
        name if name.is_empty() => "tool".to_owned(),
        name => name,
    }
}

/// The one line worth reading about a tool call.
///
/// A command line if there is one, otherwise whichever field actually says
/// what the tool is about to touch. Serialising the whole input is the last
/// resort rather than the first: an Edit carries the entire old and new text
/// of a file, and a screen of JSON tells the person across the room nothing
/// they can decide on.
fn describe(input: &kobo_json::Value) -> String {
    for name in [
        "command",
        "file_path",
        "path",
        "url",
        "pattern",
        "notebook_path",
    ] {
        if let Some(text) = input.get(name).and_then(kobo_json::Value::as_str) {
            return text.to_owned();
        }
    }
    input.to_json()
}

/// The decision as `PermissionRequest` hooks report it, or `None` for
/// "print nothing and let the terminal prompt have it".
///
/// Claude Code and Codex document the same event and the same reply, so
/// there is one shape rather than a dialect each. `message` is only read on
/// a denial, and saying where the answer came from is worth the two words
/// wherever it is shown.
#[must_use]
pub fn decision_json(_agent: &str, decision: &str) -> Option<String> {
    if decision != "allow" && decision != "deny" {
        return None;
    }
    let verdict = kobo_json::ObjectBuilder::new()
        .set("behavior", decision)
        .set("message", "Decided on the Kobo")
        .build();
    Some(
        kobo_json::ObjectBuilder::new()
            .set(
                "hookSpecificOutput",
                kobo_json::ObjectBuilder::new()
                    .set("hookEventName", "PermissionRequest")
                    .set("decision", verdict)
                    .build(),
            )
            .build()
            .to_json(),
    )
}

/// Prints the registration, ready to paste, for anyone who would rather
/// edit their own files.
///
/// # Errors
///
/// Only for an agent this binary does not know.
pub fn print_setup(id: &str) -> Result<(), String> {
    let agent = agents::find(id)?;
    let binary = agents::own_path();
    let agents::Outcome::Write(text) = agent.merge(None, &binary)? else {
        unreachable!("an empty file has nothing registered in it");
    };
    println!("Add to ~/{}:\n", agent.config);
    print!("{text}");
    println!(
        "\nOr let this write it for you:\n  {binary} setup {}",
        agent.id
    );
    Ok(())
}

/// Registers the hook with one agent, or with every one that is installed.
///
/// # Errors
///
/// For an unknown agent, or a configuration file that cannot be read,
/// parsed or written.
pub fn setup(id: Option<&str>, dry_run: bool) -> Result<(), String> {
    if let Some(id) = id {
        agents::install(agents::find(id)?, dry_run)?;
        return Ok(());
    }
    let installed: Vec<&agents::Agent> = agents::AGENTS
        .iter()
        .filter(|agent| agent.is_installed())
        .collect();
    if installed.is_empty() {
        println!("No supported coding agent found here. Looked for:");
        for agent in agents::AGENTS {
            println!("  {:<12} ~/{}", agent.name, agent.config);
        }
        return Ok(());
    }
    for agent in installed {
        agents::install(agent, dry_run)?;
    }
    if dry_run {
        println!("\nNothing was written. Run without --dry-run to do it.");
    } else {
        println!("\nStart the daemon with 'kobo-sidekickd run'.");
    }
    Ok(())
}

/// Says, for every supported agent, whether it is here and whether it asks
/// us yet.
///
/// # Errors
///
/// Only when there is no home directory to look in.
pub fn list() -> Result<(), String> {
    let binary = agents::own_path();
    for agent in agents::AGENTS {
        let state = if agent.is_installed() {
            let path = agent.config_path()?;
            let current = std::fs::read_to_string(&path).ok();
            match agent.merge(current.as_deref(), &binary) {
                Ok(agents::Outcome::AlreadyThere) => "asks this Kobo".to_owned(),
                Ok(agents::Outcome::Write(_)) => "installed, not registered yet".to_owned(),
                Err(_) => format!("installed, but ~/{} does not parse", agent.config),
            }
        } else {
            "not installed".to_owned()
        };
        println!("{:<12} {state}", agent.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        always, answered, decision_json, describe, describe_suggestion, session_identity, string,
        tool_name,
    };

    /// What the reader would be shown for one event.
    fn shown(event: &str) -> (String, String) {
        let event = kobo_json::parse(event).expect("valid json");
        let detail = event.get("tool_input").map_or_else(String::new, describe);
        (tool_name(&event), detail)
    }

    #[test]
    fn a_codex_shell_request_becomes_an_ask_with_the_command_line() {
        let (tool, detail) =
            shown(r#"{"tool_name":"shell","tool_input":{"command":"rm -rf ./build"}}"#);
        assert_eq!(tool, "shell");
        assert_eq!(detail, "rm -rf ./build");
    }

    #[test]
    fn session_identity_uses_workspace_basename_and_short_session_tail() {
        let event = kobo_json::parse(r#"{"cwd":"/work/cobalt","session_id":"01HZY2ABCD"}"#)
            .expect("valid json");
        assert_eq!(session_identity(&event), "cobalt · ABCD");
        assert_eq!(
            session_identity(&kobo_json::parse(r#"{"tool_name":"Bash"}"#).expect("valid json")),
            ""
        );
    }

    #[test]
    fn an_edit_shows_the_file_it_touches_rather_than_a_screen_of_json() {
        let event = r#"{"tool_name":"Edit","tool_input":{"file_path":"/etc/hosts",
            "old_string":"a very long file","new_string":"an even longer one"}}"#;
        let (tool, detail) = shown(event);
        assert_eq!(tool, "Edit");
        assert_eq!(detail, "/etc/hosts");
    }

    #[test]
    fn an_event_naming_no_tool_still_has_something_to_put_on_the_panel() {
        let (tool, detail) = shown("{}");
        assert_eq!(tool, "tool");
        assert_eq!(detail, "");
    }

    #[test]
    fn both_agents_are_answered_in_the_one_shape_they_share() {
        for agent in ["codex", "claude"] {
            let reply = decision_json(agent, "allow").expect("a decision");
            assert!(
                reply.contains("\"hookEventName\":\"PermissionRequest\""),
                "{reply}"
            );
            assert!(reply.contains("\"behavior\":\"allow\""), "{reply}");
        }
        let denied = decision_json("claude", "deny").expect("a decision");
        assert!(denied.contains("\"behavior\":\"deny\""), "{denied}");
        assert!(denied.contains("Decided on the Kobo"), "{denied}");
    }

    #[test]
    fn a_pass_prints_nothing_so_the_terminal_prompt_takes_over() {
        assert_eq!(decision_json("codex", "pass"), None);
        assert_eq!(decision_json("claude", "pass"), None);
        assert_eq!(decision_json("codex", "gibberish"), None);
    }

    #[test]
    fn an_always_allow_line_reads_as_words_rather_than_as_a_rule() {
        let entry = kobo_json::parse(
            r#"{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"rm -rf node_modules"}],
                "behavior":"allow","destination":"localSettings"}"#,
        )
        .expect("valid json");
        let (label, description) = describe_suggestion(&entry).expect("words");
        assert_eq!(label, "Always allow");
        assert_eq!(description, "Bash rm -rf node_modules, in this project");
    }

    #[test]
    fn accepting_edits_for_the_session_says_so_in_those_words() {
        let entry =
            kobo_json::parse(r#"{"type":"setMode","mode":"acceptEdits","destination":"session"}"#)
                .expect("valid json");
        let (label, description) = describe_suggestion(&entry).expect("words");
        assert_eq!(label, "Accept edits");
        assert_eq!(description, "for the rest of this session");
    }

    #[test]
    fn a_suggestion_with_no_sensible_wording_is_not_offered() {
        for entry in [
            r#"{"type":"addRules","rules":[],"behavior":"allow","destination":"session"}"#,
            r#"{"type":"setMode","mode":"somethingNew","destination":"session"}"#,
            r#"{"type":"aTypeInventedNextYear","destination":"session"}"#,
        ] {
            let entry = kobo_json::parse(entry).expect("valid json");
            assert_eq!(describe_suggestion(&entry), None, "{entry:?}");
        }
    }

    #[test]
    fn pressing_an_always_allow_grants_this_one_and_adopts_the_rule() {
        let entry =
            kobo_json::parse(r#"{"type":"setMode","mode":"acceptEdits","destination":"session"}"#)
                .expect("valid json");
        let reply = always(entry);
        assert!(reply.contains(r#""behavior":"allow""#), "{reply}");
        assert!(
            reply.contains(r#""updatedPermissions":[{"type":"setMode""#),
            "{reply}"
        );
    }

    #[test]
    fn an_answered_question_goes_back_as_the_tools_own_input() {
        let questions = kobo_json::parse(
            r#"[{"question":"How much detail?","header":"Detail",
                "options":[{"label":"Summary","description":"The short version"}],
                "multiSelect":false}]"#,
        )
        .expect("valid json");
        let updated = kobo_json::ObjectBuilder::new()
            .set("questions", questions)
            .set(
                "answers",
                kobo_json::Value::Object(vec![(
                    "How much detail?".to_owned(),
                    kobo_json::Value::String("Summary".to_owned()),
                )]),
            )
            .build();
        let reply = answered(updated);
        // Allowing alone is documented as not enough for this tool: the
        // answers have to ride back as the input.
        assert!(reply.contains(r#""hookEventName":"PreToolUse""#), "{reply}");
        assert!(reply.contains(r#""permissionDecision":"allow""#), "{reply}");
        assert!(
            reply.contains(r#""answers":{"How much detail?":"Summary"}"#),
            "{reply}"
        );
        // The questions come back whole, which the tool requires.
        assert!(reply.contains(r#""multiSelect":false"#), "{reply}");
        assert!(reply.contains(r#""header":"Detail""#), "{reply}");
    }

    #[test]
    fn a_missing_field_reads_as_empty_rather_than_as_the_word_null() {
        let event = kobo_json::parse(r#"{"decision":"chose"}"#).expect("valid json");
        assert_eq!(string(&event, "decision"), "chose");
        assert_eq!(string(&event, "label"), "");
    }

    #[test]
    fn a_multi_select_answer_is_a_list_and_a_single_one_is_not() {
        // Claude Code documents an array for multiSelect and a plain string
        // otherwise, so both are written the way they are documented.
        let one = kobo_json::Value::String("Summary".to_owned());
        let many = kobo_json::Value::Array(vec![
            kobo_json::Value::String("Introduction".to_owned()),
            kobo_json::Value::String("Conclusion".to_owned()),
        ]);
        let updated = kobo_json::ObjectBuilder::new()
            .set("questions", kobo_json::Value::Array(Vec::new()))
            .set(
                "answers",
                kobo_json::Value::Object(vec![
                    ("How much detail?".to_owned(), one),
                    ("Which sections?".to_owned(), many),
                ]),
            )
            .build();
        let reply = answered(updated);
        assert!(reply.contains(r#""How much detail?":"Summary""#), "{reply}");
        assert!(
            reply.contains(r#""Which sections?":["Introduction","Conclusion"]"#),
            "{reply}"
        );
    }
}
