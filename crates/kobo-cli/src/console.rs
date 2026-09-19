//! How the CLI talks to the person running it: what their terminal can do,
//! which stream carries what, and how an exit status is meant to be read.
//!
//! Three rules hold for every command, and each one has tests:
//!
//! * Progress is stderr, results are stdout. A pipe that captures stdout
//!   holds the answer and nothing else.
//! * Machine output is versioned. `--json` prints a single JSON object on
//!   stdout whose `version` field says which shape it is; a consumer that
//!   does not recognise the version stops instead of guessing.
//! * Exit status is a category, not a message. `0` is done, [`EXIT_USAGE`]
//!   is a mistake in how the command was spelled, [`EXIT_TARGET`] is a reader
//!   or simulator that could not be reached or chosen, [`EXIT_UNSUPPORTED`]
//!   is something this build or host cannot do, and [`EXIT_FAILURE`] is
//!   everything else. A script tests the category, never the wording.

use std::env;
use std::io::{BufRead, IsTerminal, Write};

use serde_json::{json, Value};

/// Version of the machine-output object [`Console::print_json`] prints.
///
/// Bumped only when the envelope's own fields change; what each command puts
/// in `data` is that command's contract, documented next to its parser.
pub const JSON_VERSION: u32 = 1;

/// The command failed in a way no narrower category describes.
pub const EXIT_FAILURE: u8 = 1;
/// The invocation itself was wrong: an unknown command or flag, a missing
/// argument, or two options that cannot be true at once.
pub const EXIT_USAGE: u8 = 2;
/// The reader or simulator to act on could not be reached, found, or chosen
/// without guessing.
pub const EXIT_TARGET: u8 = 3;
/// The command is spelled correctly but this build, host, or reader cannot
/// do it.
pub const EXIT_UNSUPPORTED: u8 = 4;

/// Prefix an error string carries when the failure is a wrong invocation.
///
/// Errors have been plain strings since the first command, and the string is
/// how the category travels from the command that knows what went wrong to
/// `main`, which owns the process exit. [`category_of`] reads the prefix and
/// [`display`] strips it, so the person reading the error never sees it.
pub const USAGE_PREFIX: &str = "usage: ";
/// The same, for a target that could not be reached or chosen.
pub const TARGET_PREFIX: &str = "target: ";
/// The same, for something this build or host cannot do.
pub const UNSUPPORTED_PREFIX: &str = "unsupported: ";

/// Tags an error as a wrong invocation: the fix is in how the command was
/// spelled, and the exit status should say so.
#[must_use]
pub fn usage(message: impl Into<String>) -> String {
    format!("{USAGE_PREFIX}{}", message.into())
}

/// Tags an error as a target problem: the reader or simulator to act on
/// could not be reached, found, or chosen without guessing.
#[must_use]
pub fn target(message: impl Into<String>) -> String {
    format!("{TARGET_PREFIX}{}", message.into())
}

/// Tags an error as beyond this build or host: the command is spelled
/// correctly and still cannot run here.
#[must_use]
pub fn unsupported(message: impl Into<String>) -> String {
    format!("{UNSUPPORTED_PREFIX}{}", message.into())
}

/// Reads the category an error was tagged with.
///
/// Usage errors predate the tags: their messages have always started with
/// "usage:" and unknown commands with "unknown command", and both are read
/// as [`EXIT_USAGE`] without rewriting every usage line in one pass.
#[must_use]
pub fn category_of(error: &str) -> u8 {
    if error.starts_with(USAGE_PREFIX) || error.starts_with("unknown command") {
        EXIT_USAGE
    } else if error.starts_with(TARGET_PREFIX) {
        EXIT_TARGET
    } else if error.starts_with(UNSUPPORTED_PREFIX) {
        EXIT_UNSUPPORTED
    } else {
        EXIT_FAILURE
    }
}

/// The error as the person should read it, with its category tag removed.
#[must_use]
pub fn display(error: &str) -> &str {
    for prefix in [TARGET_PREFIX, UNSUPPORTED_PREFIX] {
        if let Some(rest) = error.strip_prefix(prefix) {
            return rest;
        }
    }
    error
}

/// The narrowest terminal the wrap still respects. Below this, columns are
/// scarce enough that every line is already its own paragraph.
const MINIMUM_WIDTH: usize = 40;
/// The widest line the CLI wraps for. A 240-column window gets the same text
/// as a 100-column one, because prose set that wide is harder to read.
const MAXIMUM_WIDTH: usize = 100;
/// The width assumed when the terminal does not say.
const DEFAULT_WIDTH: usize = 80;

/// What the terminal in front of the command can do.
///
/// Built per render rather than cached for the process, so a window resized
/// mid-command is honoured on the next line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Console {
    /// Colour and emphasis escapes may be sent. Nothing here emits any yet;
    /// the capability is decided in one place so that when one does, it
    /// cannot reach a terminal that asked not to see it.
    color: bool,
    /// Columns the next wrapped line should fit.
    width: usize,
    /// Stdout is a terminal, so human layout (including wrapping) applies.
    /// A pipe gets full lines, because a hard wrap corrupts whatever reads
    /// the other end.
    terminal_out: bool,
    /// Stdin and stdout are both terminals, so a prompt can be answered.
    interactive: bool,
    /// Movement on screen is welcome: spinners and in-place redraws.
    motion: bool,
}

impl Console {
    /// Reads the real environment and terminal streams.
    pub fn detect() -> Self {
        Self::from_env(
            |name| env::var(name).ok(),
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
            std::io::stderr().is_terminal(),
        )
    }

    /// Decides capabilities from explicit inputs, so the matrix of
    /// `NO_COLOR`, `TERM=dumb`, widths, pipes and reduced motion is testable
    /// without a terminal.
    fn from_env(
        var: impl Fn(&str) -> Option<String>,
        stdin_terminal: bool,
        stdout_terminal: bool,
        stderr_terminal: bool,
    ) -> Self {
        let dumb = var("TERM").is_some_and(|term| term == "dumb");
        // NO_COLOR: any present, non-empty value asks for no colour.
        let no_color = var("NO_COLOR").is_some_and(|value| !value.is_empty());
        let reduced = ["KOBO_REDUCED_MOTION", "REDUCED_MOTION"]
            .iter()
            .any(|name| var(name).is_some_and(|value| !value.is_empty() && value != "0"));
        let width = var("COLUMNS")
            .and_then(|columns| columns.parse::<usize>().ok())
            .map_or(DEFAULT_WIDTH, |columns| {
                columns.clamp(MINIMUM_WIDTH, MAXIMUM_WIDTH)
            });
        Self {
            color: stderr_terminal && !dumb && !no_color,
            width,
            terminal_out: stdout_terminal,
            interactive: stdin_terminal && stdout_terminal,
            motion: stderr_terminal && !dumb && !reduced,
        }
    }

    /// Whether a prompt can be answered: stdin and stdout are both
    /// terminals. A command that needs answers and does not have this must
    /// stop with a usage error rather than block on input that never comes.
    pub fn interactive(&self) -> bool {
        self.interactive
    }

    /// Reports work in progress on stderr, where a stdout pipe never sees
    /// it.
    pub fn progress(&self, message: &str) {
        eprintln!("{message}");
    }

    /// The one machine-readable object a `--json` run prints: the envelope
    /// version, the command answering, and that command's data.
    #[must_use]
    pub fn json_envelope(command: &str, data: Value) -> String {
        json!({
            "version": JSON_VERSION,
            "command": command,
            "data": data,
        })
        .to_string()
    }

    /// Prints that object on stdout. Nothing else may go to stdout in the
    /// same run, or the consumer parses prose as JSON.
    pub fn print_json(&self, command: &str, data: Value) {
        println!("{}", Self::json_envelope(command, data));
    }

    /// Wraps prose to the terminal width, preserving blank lines and the
    /// indentation a line already has. Lines that already fit pass through
    /// byte for byte, so aligned columns keep their alignment; only
    /// over-long lines are re-flowed. A pipe gets the text unchanged.
    #[must_use]
    pub fn wrap(&self, text: &str) -> String {
        if !self.terminal_out {
            return text.to_owned();
        }
        wrap_text(text, self.width)
    }
}

/// Re-flows the lines of `text` that exceed `width`; the rest pass through
/// unchanged.
fn wrap_text(text: &str, width: usize) -> String {
    let mut wrapped = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            wrapped.push('\n');
        }
        wrap_line(line, width, &mut wrapped);
    }
    wrapped
}

fn wrap_line(line: &str, width: usize, out: &mut String) {
    if line.len() <= width {
        out.push_str(line);
        return;
    }
    let indent = line.len() - line.trim_start().len();
    let mut column = 0;
    for word in line.split_whitespace() {
        if column == 0 {
            out.push_str(&line[..indent]);
            column = indent;
            out.push_str(word);
            column += word.len();
        } else if column + 1 + word.len() > width {
            out.push('\n');
            out.push_str(&line[..indent]);
            out.push_str(word);
            column = indent + word.len();
        } else {
            out.push(' ');
            out.push_str(word);
            column += 1 + word.len();
        }
    }
}

/// Asks for one of a numbered list and returns its index, or `None` when
/// the person cancels with a blank line or the input ends.
///
/// The whole exchange is plain lines: no cursor movement, no escapes, no
/// clearing. It reads the same on a dumb terminal, through a screen reader,
/// and over a serial cable, which is exactly when the fancy alternative is
/// not an alternative at all.
pub fn choose_numbered(
    input: &mut impl BufRead,
    output: &mut impl Write,
    options: &[impl AsRef<str>],
    prompt: &str,
) -> Result<Option<usize>, String> {
    for (index, option) in options.iter().enumerate() {
        writeln!(output, "{}. {}", index + 1, option.as_ref()).map_err(|e| e.to_string())?;
    }
    loop {
        write!(output, "{prompt}")
            .and_then(|()| output.flush())
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let picked = line
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .filter(|index| *index < options.len());
        if let Some(index) = picked {
            return Ok(Some(index));
        }
        writeln!(output, "Enter a number from 1 to {}.", options.len())
            .map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn exit_categories_are_stable_and_tags_strip_for_display() {
        assert_eq!(EXIT_FAILURE, 1);
        assert_eq!(EXIT_USAGE, 2);
        assert_eq!(EXIT_TARGET, 3);
        assert_eq!(EXIT_UNSUPPORTED, 4);
        assert_eq!(category_of("usage: kobo wait --device <host>"), EXIT_USAGE);
        assert_eq!(category_of("unknown command 'nonsense'"), EXIT_USAGE);
        assert_eq!(category_of(&target("no reader answered")), EXIT_TARGET);
        assert_eq!(
            category_of(&unsupported("not compiled in")),
            EXIT_UNSUPPORTED
        );
        assert_eq!(category_of("disk full"), EXIT_FAILURE);
        assert_eq!(display(&target("no reader answered")), "no reader answered");
        assert_eq!(display(&unsupported("not compiled in")), "not compiled in");
        assert_eq!(display("usage: stays visible"), "usage: stays visible");
    }

    #[test]
    fn no_color_dumb_terminal_and_pipes_switch_off_emphasis() {
        let plain = env(&[]);
        let console = Console::from_env(&plain, true, true, true);
        assert!(console.color);
        assert!(console.interactive);
        assert!(console.motion);

        let no_color = Console::from_env(env(&[("NO_COLOR", "1")]), true, true, true);
        assert!(!no_color.color);
        assert!(no_color.motion, "NO_COLOR is about colour, not motion");
        let dumb = Console::from_env(env(&[("TERM", "dumb")]), true, true, true);
        assert!(!dumb.color);
        assert!(!dumb.motion);
        let piped = Console::from_env(&plain, false, false, false);
        assert!(!piped.color);
        assert!(!piped.interactive);
        assert!(!piped.motion);
        assert!(!piped.terminal_out);
    }

    #[test]
    fn reduced_motion_envs_stop_animation_but_keep_text() {
        for pairs in [
            &[("KOBO_REDUCED_MOTION", "1")][..],
            &[("REDUCED_MOTION", "true")][..],
        ] {
            let console = Console::from_env(env(pairs), true, true, true);
            assert!(!console.motion, "{pairs:?}");
            assert!(console.color, "text and colour stay: {pairs:?}");
        }
        let unset = Console::from_env(env(&[("KOBO_REDUCED_MOTION", "0")]), true, true, true);
        assert!(unset.motion, "an explicit 0 means motion is fine");
    }

    #[test]
    fn width_comes_from_columns_and_stays_inside_sane_bounds() {
        let narrow = Console::from_env(env(&[("COLUMNS", "48")]), true, true, true);
        assert_eq!(narrow.width, 48);
        let tiny = Console::from_env(env(&[("COLUMNS", "5")]), true, true, true);
        assert_eq!(tiny.width, MINIMUM_WIDTH);
        let huge = Console::from_env(env(&[("COLUMNS", "400")]), true, true, true);
        assert_eq!(huge.width, MAXIMUM_WIDTH);
        let garbage = Console::from_env(env(&[("COLUMNS", "wide")]), true, true, true);
        assert_eq!(garbage.width, DEFAULT_WIDTH);
    }

    #[test]
    fn wrap_refits_long_lines_and_leaves_fitting_lines_alone() {
        let console = Console::from_env(env(&[("COLUMNS", "30")]), true, true, true);
        assert_eq!(
            console.width, MINIMUM_WIDTH,
            "narrower than the floor clamps"
        );
        let wrapped = console.wrap("results print on stdout and progress prints on stderr always");
        assert_eq!(
            wrapped,
            "results print on stdout and progress\nprints on stderr always"
        );
        let aligned = "  new <name>             Create a Rust application";
        let wide = Console::from_env(env(&[]), true, true, true);
        assert_eq!(
            wide.wrap(aligned),
            aligned,
            "a line that fits keeps its alignment byte for byte"
        );
        assert_eq!(console.wrap("one\n\nthree"), "one\n\nthree");
        let indented =
            console.wrap("    continuation lines keep their own indentation when wrapped");
        for line in indented.lines() {
            assert!(line.len() <= MINIMUM_WIDTH, "{line:?}");
            assert!(line.starts_with("    "), "{line:?}");
        }
    }

    #[test]
    fn a_pipe_gets_the_text_unchanged() {
        let piped = Console::from_env(env(&[("COLUMNS", "20")]), false, false, false);
        let long = "results print on stdout and progress prints on stderr";
        assert_eq!(piped.wrap(long), long);
    }

    #[test]
    fn json_envelope_carries_version_command_and_data() {
        let envelope: Value = serde_json::from_str(&Console::json_envelope(
            "devices",
            json!({"readers": ["192.0.2.10"], "other_hosts": 2}),
        ))
        .unwrap();
        assert_eq!(envelope["version"], JSON_VERSION);
        assert_eq!(envelope["command"], "devices");
        assert_eq!(envelope["data"]["readers"][0], "192.0.2.10");
        assert_eq!(envelope["data"]["other_hosts"], 2);
    }

    #[test]
    fn numbered_choices_pick_retry_and_cancel() {
        let options = ["Set up a reader", "Preview photos", "Exit"];
        let mut output = Vec::new();
        let picked = choose_numbered(&mut &b"2\n"[..], &mut output, &options, "Choose: ")
            .unwrap()
            .unwrap();
        assert_eq!(picked, 1);
        let printed = String::from_utf8(output).unwrap();
        assert!(printed.contains("1. Set up a reader\n2. Preview photos\n3. Exit\n"));
        assert!(!printed.contains('\u{1b}'), "plain lines only");

        let mut output = Vec::new();
        let picked = choose_numbered(
            &mut &b"0\n99\nabc\n1\n"[..],
            &mut output,
            &options,
            "Choose: ",
        )
        .unwrap()
        .unwrap();
        assert_eq!(picked, 0);
        let printed = String::from_utf8(output).unwrap();
        assert_eq!(printed.matches("Enter a number from 1 to 3.").count(), 3);

        for input in ["", "\n"] {
            assert!(
                choose_numbered(&mut input.as_bytes(), &mut Vec::new(), &options, "Choose: ")
                    .unwrap()
                    .is_none()
            );
        }
    }
}
