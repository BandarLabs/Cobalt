//! Which coding agents exist, where they keep their configuration, and how
//! each one spells "run this command when you want permission".
//!
//! Every agent here asks the same question and accepts the same three
//! answers. What differs is only paperwork: the file the registration lives
//! in, the name of the event, and whether the command is written as a string
//! or as a list. That is a table, not a set of branches, so it is one -- and
//! adding an agent is a row rather than a rewrite of `setup`.
//!
//! Writing into somebody else's configuration file is the delicate part. The
//! rules are: never write a file that did not parse, keep a copy of what was
//! there before, leave every key that is not ours exactly as it was, and do
//! nothing at all if the hook is already registered. Running `setup` twice
//! is meant to be as boring as running it once.

use kobo_json::Value;
use std::path::PathBuf;

/// How an agent spells a hook registration in its configuration file.
///
/// Claude Code and Codex landed on the same shape, so there is one variant
/// for both. That is worth stating rather than assuming: Codex's `command`
/// is a string and a list is refused outright, which the binary says in as
/// many words when you hand it one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wiring {
    /// `{ "hooks": { EVENT: [ { "matcher": "*", "hooks": [ ... ] } ] } }`,
    /// where each hook is a `type`, a `command` string and a `timeout`.
    Matcher,
}

/// One coding agent, described once and used by detection, setup and the
/// hook itself.
#[derive(Debug)]
pub struct Agent {
    /// What the user types: `setup claude`.
    pub id: &'static str,
    /// What the reader shows above the question.
    pub name: &'static str,
    /// The configuration file, relative to the home directory.
    pub config: &'static str,
    /// A directory whose presence means this agent has run here.
    pub marker: &'static str,
    /// Executables that mean this agent is installed.
    pub binaries: &'static [&'static str],
    /// The hook event that means "may I?".
    pub event: &'static str,
    /// Whether that event fires before every tool call rather than only when
    /// the agent actually wants permission.
    ///
    /// Claude Code's `PreToolUse` fires for everything, so a hook that
    /// forwarded all of it would ring the reader for every `grep` and
    /// `wc -l` the agent ran, including the ones it was never going to ask
    /// about. Codex's `PermissionRequest` fires only when it has a real
    /// question, so nothing needs filtering there and filtering would in
    /// fact throw away questions that were meant for us.
    pub every_tool: bool,
    /// How its configuration file is shaped.
    pub wiring: Wiring,
}

/// Every agent this daemon can answer for.
pub const AGENTS: &[Agent] = &[
    Agent {
        id: "claude",
        name: "Claude Code",
        config: ".claude/settings.json",
        marker: ".claude",
        binaries: &["claude"],
        event: "PreToolUse",
        every_tool: true,
        wiring: Wiring::Matcher,
    },
    Agent {
        id: "codex",
        name: "Codex",
        config: ".codex/hooks.json",
        marker: ".codex",
        binaries: &["codex"],
        event: "PermissionRequest",
        every_tool: false,
        wiring: Wiring::Matcher,
    },
];

/// The agent registered under `id`.
///
/// # Errors
///
/// When no agent goes by that name, with the list of those that do.
pub fn find(id: &str) -> Result<&'static Agent, String> {
    AGENTS.iter().find(|agent| agent.id == id).ok_or_else(|| {
        let known: Vec<&str> = AGENTS.iter().map(|agent| agent.id).collect();
        format!("unknown agent '{id}'; expected one of {}", known.join(", "))
    })
}

impl Agent {
    /// The absolute path of this agent's configuration file.
    ///
    /// # Errors
    ///
    /// Only when there is no `HOME` to resolve it against.
    pub fn config_path(&self) -> Result<PathBuf, String> {
        Ok(home()?.join(self.config))
    }

    /// Whether this agent appears to be installed: either it has left its
    /// directory in the home directory or its executable is on the path.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        let marked = home().is_ok_and(|home| home.join(self.marker).is_dir());
        marked || self.binaries.iter().any(|name| on_path(name))
    }
}

/// The home directory.
fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "no HOME in the environment".to_owned())
}

/// Whether `name` is an executable somewhere on `PATH`.
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
}

/// What `setup` found when it looked at a configuration file.
#[derive(PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The hook was already registered. Nothing was written.
    AlreadyThere,
    /// The file needs replacing with this text.
    Write(String),
}

impl Agent {
    /// Our own registration, in this agent's dialect.
    fn entry(&self, binary: &str) -> Value {
        let hook = kobo_json::ObjectBuilder::new()
            .set("type", "command")
            // One string. Codex refuses a list with "invalid type: sequence,
            // expected a string", and Claude Code has never taken one.
            .set("command", format!("{binary} hook {}", self.id))
            // Longer than the daemon holds a question, so the agent waits for
            // the person rather than giving up on them.
            .set("timeout", 360)
            .build();
        match self.wiring {
            Wiring::Matcher => kobo_json::ObjectBuilder::new()
                .set("matcher", "*")
                .set("hooks", Value::Array(vec![hook]))
                .build(),
        }
    }

    /// The configuration file's new text, or [`Outcome::AlreadyThere`].
    ///
    /// `current` is the file's existing contents, or `None` when there is no
    /// file yet. Everything already in it is carried across untouched; our
    /// entry joins whatever hooks were registered before rather than
    /// replacing them, because another tool's hook is not ours to remove.
    ///
    /// # Errors
    ///
    /// When the file exists but is not JSON we can read. That is deliberately
    /// fatal: a configuration file we cannot parse is one we must not
    /// rewrite, because doing so would throw away whatever it really said.
    pub fn merge(&self, current: Option<&str>, binary: &str) -> Result<Outcome, String> {
        let root = match current.map(str::trim) {
            None | Some("") => Value::Object(Vec::new()),
            Some(text) => kobo_json::parse(text).map_err(|error| {
                format!(
                    "{} is not JSON this can read ({error}).\n\
                     Nothing was written. Comments and trailing commas are the \
                     usual reason.\n\
                     Fix the file, or register the hook by hand:\n  \
                     {binary} setup {} --print",
                    self.config, self.id
                )
            })?,
        };
        let hooks = root
            .get("hooks")
            .cloned()
            .unwrap_or(Value::Object(Vec::new()));
        let existing = hooks
            .get(self.event)
            .and_then(Value::as_array)
            .map(<[Value]>::to_vec)
            .unwrap_or_default();
        if existing
            .iter()
            .any(|entry| mentions(entry, "kobo-sidekickd"))
        {
            return Ok(Outcome::AlreadyThere);
        }
        let mut entries = existing;
        entries.push(self.entry(binary));
        let hooks = set_field(&hooks, self.event, Value::Array(entries));
        let root = set_field(&root, "hooks", hooks);
        Ok(Outcome::Write(root.to_json_pretty() + "\n"))
    }
}

/// `object` with `key` set, keeping every other field where it was.
///
/// An existing key keeps its position rather than moving to the end, so a
/// file's diff after this is the smallest true one.
fn set_field(object: &Value, key: &str, value: Value) -> Value {
    let mut fields = match object {
        Value::Object(fields) => fields.clone(),
        _ => Vec::new(),
    };
    if let Some(slot) = fields.iter_mut().find(|(name, _)| name == key) {
        slot.1 = value;
    } else {
        fields.push((key.to_owned(), value));
    }
    Value::Object(fields)
}

/// Whether any string anywhere inside `value` contains `needle`.
///
/// Used to recognise our own registration however the agent reshaped it, and
/// whatever path the binary was at when it was written.
fn mentions(value: &Value, needle: &str) -> bool {
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::String(text) if text.contains(needle) => return true,
            Value::Array(items) => stack.extend(items.iter()),
            Value::Object(fields) => stack.extend(fields.iter().map(|(_, value)| value)),
            _ => {}
        }
    }
    false
}

/// This executable's path, for writing into somebody's configuration.
///
/// An agent runs the hook with no shell and no inherited working directory,
/// so the registration has to name an absolute path rather than trust that
/// `kobo-sidekickd` will be found.
#[must_use]
pub fn own_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_owned))
        .unwrap_or_else(|| "kobo-sidekickd".to_owned())
}

/// Registers the hook with one agent, writing its configuration file.
///
/// Keeps a copy of whatever was there as `<file>.bak` before replacing it,
/// and says in one line what it did.
///
/// # Errors
///
/// When the file cannot be read, cannot be parsed, or cannot be written.
pub fn install(agent: &Agent, dry_run: bool) -> Result<bool, String> {
    let path = agent.config_path()?;
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let outcome = agent.merge(current.as_deref(), &own_path())?;
    let Outcome::Write(text) = outcome else {
        println!("{:<12} already registered", agent.name);
        return Ok(false);
    };
    if dry_run {
        println!("{:<12} would write {}", agent.name, path.display());
        return Ok(true);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let saved = if current.is_some() {
        let backup = path.with_extension("json.bak");
        std::fs::copy(&path, &backup)
            .map_err(|error| format!("cannot back up {}: {error}", path.display()))?;
        format!(", previous kept as {}", backup.display())
    } else {
        String::new()
    };
    std::fs::write(&path, text)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    println!("{:<12} registered in {}{saved}", agent.name, path.display());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{find, Outcome};

    /// The path an agent would be given, which is never the bare name.
    const BINARY: &str = "/opt/kobo-sidekickd";

    fn written(id: &str, current: Option<&str>) -> String {
        match find(id).expect("known agent").merge(current, BINARY) {
            Ok(Outcome::Write(text)) => text,
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn a_home_with_no_configuration_yet_gets_a_whole_file() {
        let text = written("claude", None);
        let parsed = kobo_json::parse(&text).expect("what we write is JSON");
        let entry = parsed
            .get("hooks")
            .and_then(|hooks| hooks.get("PreToolUse"))
            .and_then(|event| event.index(0))
            .expect("one registration");
        assert_eq!(
            entry.get("matcher").and_then(kobo_json::Value::as_str),
            Some("*")
        );
        let hook = entry
            .get("hooks")
            .and_then(|hooks| hooks.index(0))
            .expect("one hook");
        assert_eq!(
            hook.get("command").and_then(kobo_json::Value::as_str),
            Some("/opt/kobo-sidekickd hook claude")
        );
        assert_eq!(
            hook.get("timeout").and_then(kobo_json::Value::as_i64),
            Some(360)
        );
    }

    /// Codex refuses a list outright. Its own words, handed one: "invalid
    /// type: sequence, expected a string".
    #[test]
    fn codex_is_given_its_command_as_one_string_because_a_list_is_refused() {
        let text = written("codex", None);
        let parsed = kobo_json::parse(&text).expect("what we write is JSON");
        let command = parsed
            .get("hooks")
            .and_then(|hooks| hooks.get("PermissionRequest"))
            .and_then(|event| event.index(0))
            .and_then(|entry| entry.get("hooks"))
            .and_then(|hooks| hooks.index(0))
            .and_then(|hook| hook.get("command"))
            .expect("a command");
        assert_eq!(
            command.as_str(),
            Some("/opt/kobo-sidekickd hook codex"),
            "a sequence here is the one shape Codex rejects"
        );
        assert!(command.as_array().is_none(), "never a list");
    }

    #[test]
    fn settings_that_have_nothing_to_do_with_us_survive_untouched() {
        let before = r#"{"model":"opus","tui":"fullscreen"}"#;
        let after = kobo_json::parse(&written("claude", Some(before))).expect("JSON");
        assert_eq!(
            after.get("model").and_then(kobo_json::Value::as_str),
            Some("opus")
        );
        assert_eq!(
            after.get("tui").and_then(kobo_json::Value::as_str),
            Some("fullscreen")
        );
    }

    /// Somebody else's hook is not ours to remove.
    #[test]
    fn another_tools_hook_is_joined_rather_than_replaced() {
        let before = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/usr/bin/other"}]}],"Stop":[{"hooks":[]}]}}"#;
        let after = kobo_json::parse(&written("claude", Some(before))).expect("JSON");
        let event = after
            .get("hooks")
            .and_then(|hooks| hooks.get("PreToolUse"))
            .and_then(kobo_json::Value::as_array)
            .expect("an array");
        assert_eq!(event.len(), 2, "the other tool kept its place");
        assert_eq!(
            event[0].get("matcher").and_then(kobo_json::Value::as_str),
            Some("Bash")
        );
        assert!(after
            .get("hooks")
            .and_then(|hooks| hooks.get("Stop"))
            .is_some());
    }

    #[test]
    fn registering_twice_changes_nothing_the_second_time() {
        let once = written("claude", None);
        let agent = find("claude").expect("known agent");
        assert_eq!(
            agent.merge(Some(&once), BINARY).expect("reads back"),
            Outcome::AlreadyThere
        );
    }

    /// Recognised by name, so moving the binary does not register it twice.
    #[test]
    fn a_registration_written_from_another_path_is_still_recognised_as_ours() {
        let elsewhere =
            written("claude", None).replace("/opt/kobo-sidekickd", "/usr/local/bin/kobo-sidekickd");
        assert_eq!(
            find("claude")
                .expect("known agent")
                .merge(Some(&elsewhere), BINARY)
                .expect("reads back"),
            Outcome::AlreadyThere
        );
    }

    /// A file we cannot read is a file we must not rewrite.
    #[test]
    fn a_configuration_that_does_not_parse_is_refused_rather_than_replaced() {
        let error = find("claude")
            .expect("known agent")
            .merge(Some("{\n  // notes\n  \"model\": \"opus\",\n}"), BINARY)
            .expect_err("refuses");
        assert!(error.contains("Nothing was written"), "{error}");
        assert!(error.contains(".claude/settings.json"), "{error}");
    }

    #[test]
    fn an_empty_file_is_treated_as_no_file_rather_than_as_broken() {
        assert!(matches!(
            find("claude")
                .expect("known agent")
                .merge(Some("  \n"), BINARY),
            Ok(Outcome::Write(_))
        ));
    }

    #[test]
    fn an_agent_nobody_has_heard_of_is_named_along_with_those_we_know() {
        let error = find("emacs").expect_err("unknown");
        assert!(
            error.contains("claude") && error.contains("codex"),
            "{error}"
        );
    }
}
