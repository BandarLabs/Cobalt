//! A shell on the panel.
//!
//! This replaced the hello example, which showed that a button could be
//! tapped and nothing else. A terminal is the opposite: it is the hardest
//! thing this platform can host, and every part of it is a claim that has to
//! be true.
//!
//! What it demonstrates:
//!
//! - **A capability the application does not hold.** There is no pseudo-
//!   terminal here, no fork, no file descriptor and no path to a program. The
//!   application says what was typed and the runtime decides whether there is
//!   a shell at all. An application without [`kobo_sdk::Capability::Shell`]
//!   running this same code is refused and shown saying so.
//! - **A grid measured rather than assumed.** [`kobo_sdk::terminal_grid_for`]
//!   lays this screen out with an empty terminal and measures what is left, so
//!   the shell wraps its lines exactly where the reader sees them wrap.
//! - **Keys that send rather than collect.** The text keyboard gathers a
//!   string and hands it over on submit, which is right for a search box and
//!   useless for a shell: `Ctrl-C` has to arrive while the program is still
//!   running.
//! - **Background life.** Leaving the terminal does not end it. A build
//!   started here keeps running while the reader is elsewhere, and coming back
//!   shows what it printed.
//!
//! ## Why nothing here is dangerous by accident
//!
//! On this device a shell is root on a writable root filesystem, which makes
//! it the first thing the platform hosts that a reboot cannot undo. That risk
//! is deliberately not this application's to manage: it is a capability the
//! runtime grants, so there is exactly one place to look and exactly one place
//! to change when manifests arrive.

use kobo_sdk::terminal::{TerminalKeys, Typed};
use kobo_sdk::{
    action_id, ActionId, Caret, Context, KoboApp, Screen, ScreenBuilder, ShellError, ShellEvent,
};
use kobo_term::Terminal;
use std::process::ExitCode;

/// Restarts the program after it has finished.
///
/// In the top bar rather than in the flow, because a control that appears
/// under the keys would move the keys, and the reader's finger is already
/// there.
const RESTART: &str = "restart";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Starting,
    Running,
    Ended(i32),
    Refused(ShellError),
}

impl Status {
    /// What the top bar says, which is the only place status is allowed to go.
    ///
    /// Status in the flow above the terminal would move the grid the moment it
    /// changed, and a grid that moves is a shell whose output no longer lines
    /// up with what it was told its width was.
    fn title(self) -> String {
        match self {
            Self::Starting => "Terminal".to_string(),
            Self::Running => "Terminal - sh".to_string(),
            Self::Ended(status) => format!("Terminal - exited {status}"),
            Self::Refused(ShellError::NotPermitted) => "Terminal - not permitted".to_string(),
            Self::Refused(ShellError::Unavailable) => "Terminal - unavailable here".to_string(),
            Self::Refused(error) => format!("Terminal - refused ({error:?})"),
        }
    }

    const fn finished(self) -> bool {
        matches!(self, Self::Ended(_) | Self::Refused(_))
    }
}

struct App {
    keys: TerminalKeys,
    terminal: Terminal,
    grid: (u16, u16),
    status: Status,
    /// Whether anything drawn now would actually be seen.
    ///
    /// A shell left running in the background can print megabytes. Sending a
    /// screen per chunk to a runtime that is showing something else is traffic
    /// for no picture, so the screen is rebuilt once on the way back instead.
    visible: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            keys: TerminalKeys::new(),
            // Replaced in `on_start` once the panel has been measured. A
            // terminal always exists so that output arriving before or after a
            // program has any home to go to.
            terminal: Terminal::new(1, 1),
            grid: (0, 0),
            status: Status::Starting,
            visible: true,
        }
    }
}

impl App {
    /// The screen, given rows and a cursor.
    ///
    /// Everything above the terminal is fixed height and everything below it
    /// is the keys, so the grid is whatever is left. Nothing in the flow ever
    /// changes size, which is what keeps a key under the finger that is about
    /// to press it again.
    fn compose(&self, rows: Vec<String>, cursor: Option<Caret>) -> Screen {
        let mut builder = ScreenBuilder::new("terminal").top_bar(self.status.title());
        if self.status.finished() {
            builder = builder.top_bar_action(RESTART, "restart");
        }
        builder
            .terminal(rows, cursor)
            .terminal_keys(&self.keys)
            .build()
    }

    fn view(&self) -> Screen {
        self.compose(self.terminal.rows(), self.terminal.cursor())
    }

    /// Asks the layout engine, not arithmetic, how big the grid is.
    fn measure(&self, context: &Context) -> (u16, u16) {
        let empty = self.compose(Vec::<String>::new(), None);
        kobo_sdk::terminal_grid_for(&empty, &context.metrics())
    }

    fn repaint(&mut self, context: &mut Context) {
        if self.visible {
            let screen = self.view();
            context.set_screen(screen);
        }
    }

    fn start(&mut self, context: &mut Context) {
        let (columns, rows) = self.grid;
        if columns == 0 || rows == 0 {
            return;
        }
        self.terminal = Terminal::new(columns, rows);
        self.status = Status::Starting;
        context.shell().open(columns, rows);
    }
}

impl KoboApp for App {
    fn on_start(&mut self, context: &mut Context) {
        self.grid = self.measure(context);
        self.start(context);
        self.repaint(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(RESTART) {
            if self.status.finished() {
                self.start(context);
                self.repaint(context);
            }
            return;
        }
        let Some(typed) = self.keys.press(action) else {
            return;
        };
        match typed {
            // Sent, and nothing drawn. The answer to a keystroke is what the
            // program prints, and painting an echo of our own would either
            // duplicate it or race it.
            Typed::Send(bytes) => context.shell().input(bytes),
            // A modifier moved, so every label changed and nothing was sent.
            Typed::Changed => self.repaint(context),
        }
    }

    fn on_shell_event(&mut self, context: &mut Context, event: ShellEvent) {
        match event {
            ShellEvent::Opened => self.status = Status::Running,
            ShellEvent::Output(bytes) => self.terminal.feed(&bytes),
            ShellEvent::Closed { status } => self.status = Status::Ended(status),
            ShellEvent::Refused(error) => self.status = Status::Refused(error),
        }
        self.repaint(context);
    }

    fn on_background(&mut self, _context: &mut Context) {
        self.visible = false;
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.visible = true;
        self.repaint(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        // Not required for safety: the runtime stops the program when the
        // application goes away, precisely so that a crash cannot leave a root
        // shell running with nothing attached. Asked for anyway, because the
        // ordinary path should not depend on the backstop.
        context.shell().close();
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("terminal", App::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("terminal: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, Status, RESTART};
    use kobo_sdk::terminal::Typed;
    use kobo_sdk::{action_id, AppRunner, Command, ShellError, ShellEvent};
    use kobo_ui::{Chrome, LayoutKind, Rect, Screen, CLARA_BW_METRICS};

    fn layout(screen: &Screen) -> kobo_ui::Layout {
        screen.layout_with(&CLARA_BW_METRICS, Chrome::with_back(true))
    }

    fn screens(commands: &[Command]) -> Vec<Screen> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .collect()
    }

    fn sent(commands: &[Command]) -> Vec<Vec<u8>> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Shell(kobo_sdk::ShellRequest::Input(bytes)) => Some(bytes.clone()),
                _ => None,
            })
            .collect()
    }

    fn opened(commands: &[Command]) -> Option<(u16, u16)> {
        commands.iter().find_map(|command| match command {
            Command::Shell(kobo_sdk::ShellRequest::Open { columns, rows }) => {
                Some((*columns, *rows))
            }
            _ => None,
        })
    }

    fn started() -> (AppRunner<App>, Vec<Command>) {
        let mut runner = AppRunner::with_metrics(App::default(), CLARA_BW_METRICS);
        let commands = runner.start();
        (runner, commands)
    }

    /// The grid is the one the panel will actually draw.
    ///
    /// A shell told it has more columns than the panel shows wraps its lines
    /// somewhere the reader cannot see, so every full-screen program becomes
    /// unusable. This asserts the request matches the drawn grid rather than
    /// asserting a number somebody typed.
    #[test]
    fn the_shell_is_opened_with_the_grid_the_screen_really_has() {
        let (_runner, commands) = started();
        let (columns, rows) = opened(&commands).expect("a terminal opens a shell when it starts");
        assert!(columns > 20, "{columns} columns is not a usable terminal");
        assert!(rows > 10, "{rows} rows is not a usable terminal");

        let screen = screens(&commands).pop().expect("the first screen is drawn");
        let grid = layout(&screen)
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TerminalGrid)
            .map(|node| node.rect)
            .expect("the screen has a terminal on it");
        assert_eq!(
            kobo_sdk::terminal_grid(grid.width, grid.height),
            (columns, rows),
            "the drawn grid must be the grid the shell was told about"
        );
    }

    /// Output moves nothing.
    ///
    /// This platform has already shipped a control that moved out from under
    /// the finger that tapped it. A terminal prints constantly, so if output
    /// could resize anything the keys would move on every line.
    #[test]
    fn printing_does_not_move_the_keys() {
        let (mut runner, commands) = started();
        let before = screens(&commands).pop().expect("a first screen");
        let key = action_id("kb.r0c0");
        let rect_of = |screen: &Screen| -> Rect {
            layout(screen)
                .rect_of_action(key)
                .expect("the letter keys are always drawn")
        };
        let first = rect_of(&before);

        let commands = runner.shell_event(ShellEvent::Opened);
        let commands = [
            commands,
            runner.shell_event(ShellEvent::Output(
                b"a really quite long line of output\r\nand another\r\n".to_vec(),
            )),
        ]
        .concat();
        let after = screens(&commands).pop().expect("output redraws the screen");
        assert_eq!(first, rect_of(&after));
    }

    /// A keystroke leaves immediately and draws nothing.
    #[test]
    fn a_key_is_sent_rather_than_collected() {
        let (mut runner, _commands) = started();
        let commands = runner.action(action_id("kb.r0c0"));
        assert_eq!(sent(&commands), vec![b"q".to_vec()]);
        assert!(
            screens(&commands).is_empty(),
            "the echo is what redraws, not the keypress"
        );
    }

    /// A refusal is visible, not silent.
    ///
    /// An application that asked for a shell and was told no must say so. The
    /// alternative is a black rectangle that never fills in, which reads as a
    /// device that has stopped working.
    #[test]
    fn a_refusal_is_shown_in_the_bar() {
        let (mut runner, _commands) = started();
        let commands = runner.shell_event(ShellEvent::Refused(ShellError::NotPermitted));
        let screen = screens(&commands).pop().expect("a refusal redraws");
        let title = screen.top_bar.as_ref().map(|bar| bar.title.clone());
        assert_eq!(title.as_deref(), Some("Terminal - not permitted"));
        assert!(
            layout(&screen).rect_of_action(action_id(RESTART)).is_some(),
            "a refused terminal offers to try again"
        );
    }

    /// A program that finished can be started again without leaving.
    #[test]
    fn restarting_opens_a_second_shell() {
        let (mut runner, _commands) = started();
        let _ended = runner.shell_event(ShellEvent::Closed { status: 0 });
        let commands = runner.action(action_id(RESTART));
        assert!(
            opened(&commands).is_some(),
            "restart has to actually open a shell"
        );
    }

    /// Output arriving while the reader is elsewhere is kept, not drawn.
    #[test]
    fn a_backgrounded_terminal_keeps_reading_without_repainting() {
        let (mut runner, _commands) = started();
        let _running = runner.shell_event(ShellEvent::Opened);
        let _away = runner.lifecycle(kobo_sdk::Lifecycle::Background);
        let commands = runner.shell_event(ShellEvent::Output(b"built\r\n".to_vec()));
        assert!(
            screens(&commands).is_empty(),
            "nothing is drawn for a panel showing something else"
        );

        let commands = runner.lifecycle(kobo_sdk::Lifecycle::Foreground);
        let screen = screens(&commands).pop().expect("coming back redraws");
        let text = layout(&screen)
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::TerminalGrid)
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("built"),
            "what arrived while away must be on the screen that comes back"
        );
    }

    #[test]
    fn control_is_shown_in_its_label_because_there_is_nowhere_else() {
        let (mut runner, _commands) = started();
        let commands = runner.action(action_id("term.ctrl"));
        let screen = screens(&commands).pop().expect("a modifier redraws");
        let labels = layout(&screen)
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>();
        assert!(labels.iter().any(|label| label == "CTRL"));
    }

    #[test]
    fn a_status_says_what_happened() {
        assert_eq!(Status::Ended(1).title(), "Terminal - exited 1");
        assert!(Status::Running.title().contains("sh"));
        assert!(!Status::Starting.finished());
        assert!(Status::Ended(0).finished());
    }

    #[test]
    fn typed_bytes_are_what_a_terminal_expects() {
        // Guards the two encodings a shell notices immediately: return is a
        // carriage return, and the key above it deletes rather than moving.
        let mut keys = kobo_sdk::terminal::TerminalKeys::new();
        assert_eq!(
            keys.press(action_id("kb.enter")),
            Some(Typed::Send(b"\r".to_vec()))
        );
        assert_eq!(
            keys.press(action_id("kb.backspace")),
            Some(Typed::Send(vec![0x7f]))
        );
    }
    /// The whole loop, against a real shell.
    ///
    /// Every other test here mocks the runtime, which proves the application
    /// is consistent with an idea of a terminal rather than with one. This
    /// runs the same host the daemon runs, starts a real `/bin/sh`, types into
    /// it by tapping keys and waits for the answer to appear on the screen.
    ///
    /// It is the test that would have caught every mistake that mattered: a
    /// return that sends a newline, a backspace that moves instead of
    /// deleting, a grid the shell was never told about, or output that never
    /// reaches the panel at all.
    #[test]
    fn typing_echo_into_a_real_shell_puts_its_answer_on_the_screen() {
        use kobo_policy::Capability;
        use kobo_shell::Shells;
        use std::time::{Duration, Instant};

        let mut shells = Shells::new(&[Capability::Shell]);
        let (mut runner, commands) = started();
        let mut latest = screens(&commands).pop();

        // The runtime side of the loop: every request the application made
        // goes to the same host the daemon uses, and everything that comes
        // back goes to the application.
        let serve = |runner: &mut AppRunner<App>,
                     shells: &mut Shells,
                     commands: Vec<Command>,
                     latest: &mut Option<Screen>| {
            let mut queue = commands;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let mut next = Vec::new();
                for command in queue.drain(..) {
                    match command {
                        Command::SetScreen(screen) => *latest = Some(screen),
                        Command::Shell(request) => {
                            if let Some(event) = shells.handle(request) {
                                next.extend(runner.shell_event(event));
                            }
                        }
                        _ => {}
                    }
                }
                for event in shells.drain() {
                    next.extend(runner.shell_event(event));
                }
                if next.is_empty() {
                    return;
                }
                queue = next;
                if Instant::now() > deadline {
                    return;
                }
            }
        };

        serve(&mut runner, &mut shells, commands, &mut latest);
        assert!(shells.is_open(), "a real shell has to have started");

        for key in [
            "kb.r0c2", "kb.r2c2", "kb.r1c5", "kb.r0c8", "kb.space", "kb.r1c5", "kb.r0c7",
            "kb.enter",
        ] {
            let commands = runner.action(action_id(key));
            serve(&mut runner, &mut shells, commands, &mut latest);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let commands = runner.action(action_id("nothing at all"));
            serve(&mut runner, &mut shells, commands, &mut latest);
            let screen = latest.clone().expect("a screen exists by now");
            let text = layout(&screen)
                .nodes
                .iter()
                .filter(|node| node.kind == LayoutKind::TerminalGrid)
                .flat_map(|node| node.text_lines.clone())
                .collect::<Vec<_>>()
                .join("\n");
            // The command echoes as it is typed, so the answer is the *second*
            // occurrence. Counting is what distinguishes a shell that ran the
            // line from one still waiting for a line ending it never got.
            if text.matches("hi").count() >= 2 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the shell never answered; screen was:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
