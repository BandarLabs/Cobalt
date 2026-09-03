//! The device half of Paperterm. It renders host rows but never owns a shell.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::terminal::{TerminalKeys, Typed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Caret, Context, KoboApp, Orientation, Screen, ScreenBuilder,
    Space, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

/// One host request may wait this long before returning an unchanged screen.
const LONGEST_POLL_SECONDS: u32 = 25;
/// Failed radio requests rest before trying again, with one request in flight.
const FAILURE_SLEEP_SECONDS: u32 = 5;
/// Bounded enough that a malformed host cannot fill the app task channel.
const MAX_REPLY_BYTES: u32 = 64 * 1024;
const PAIRING: &str = "pairing";
const REPAIR: &str = "repair";
const OFF_AIR: &str = "off the air";
const PAIRING_REFUSED: &str = "Pairing was refused — run kobo stream init.";
const INPUT_REFUSED: &str = "Input was not accepted by your computer.";
const CONTROL_COLUMNS: u8 = 6;
const CONTROL_KEYS: [(&str, &str, &[u8]); 9] = [
    ("key-up", "↑", b"\x1b[A"),
    ("key-down", "↓", b"\x1b[B"),
    ("key-left", "←", b"\x1b[D"),
    ("key-right", "→", b"\x1b[C"),
    ("key-enter", "Enter", b"\r"),
    ("key-esc", "Esc", b"\x1b"),
    ("key-y", "y", b"y"),
    ("key-n", "n", b"n"),
    ("key-ctrl-c", "Ctrl-C", b"\x03"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Opening,
    Address,
    Code,
    Watching,
    Ended,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Input {
    None,
    Controls,
    Full,
}
impl Input {
    fn from_wire(text: &str) -> Self {
        match text {
            "controls" => Self::Controls,
            "full" => Self::Full,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Hello {
    Ready,
    Resize,
    Invalid,
}

struct Paperterm {
    view: View,
    keyboard: Keyboard,
    address: String,
    code: String,
    rows: Vec<String>,
    cursor: Option<Caret>,
    input: Input,
    grid_input: Input,
    session: Option<u64>,
    sequence: u64,
    poll: Option<TaskId>,
    nap: Option<TaskId>,
    send: Option<TaskId>,
    keys: TerminalKeys,
    failure: Option<String>,
}

impl Default for Paperterm {
    fn default() -> Self {
        Self {
            view: View::Opening,
            keyboard: Keyboard::new(),
            address: String::new(),
            code: String::new(),
            rows: Vec::new(),
            cursor: None,
            input: Input::None,
            grid_input: Input::Full,
            session: None,
            sequence: 0,
            poll: None,
            nap: None,
            send: None,
            keys: TerminalKeys::new(),
            failure: None,
        }
    }
}

impl Paperterm {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }
    fn screen(&self) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("paperterm-opening")
                .top_bar("Paperterm")
                .activity("Reading pairing", None)
                .build(),
            View::Address => ScreenBuilder::new("paperterm-pairing")
                .top_bar("Paperterm")
                .heading("Pair with your computer")
                .text("Run kobo stream init there, then enter the address it prints.")
                .field("address", self.keyboard.text(), "192.168.1.20:9332")
                .spacer(Space::Small)
                .keyboard(&self.keyboard, "Next")
                .build(),
            View::Code => ScreenBuilder::new("paperterm-code")
                .top_bar("Paperterm")
                .heading("Now the pairing code")
                .text("Enter the six characters printed by kobo stream init.")
                .field("code", self.keyboard.text(), "ABC123")
                .spacer(Space::Small)
                .keyboard(&self.keyboard, "Watch")
                .build(),
            View::Watching | View::Ended => {
                let mut screen = ScreenBuilder::new(if self.view == View::Ended {
                    "paperterm-ended"
                } else {
                    "paperterm-watching"
                })
                .top_bar(if self.view == View::Ended {
                    "Paperterm — ended"
                } else {
                    "Paperterm"
                })
                .top_bar_action(REPAIR, "Pairing")
                .terminal(self.rows.clone(), self.cursor);
                if let Some(failure) = &self.failure {
                    screen = screen.banner(BannerLevel::Attention, failure.clone());
                }
                if self.view == View::Ended {
                    screen = screen.secondary("The host held the final terminal screen.");
                } else {
                    screen = match self.input {
                        Input::None => screen,
                        Input::Controls => screen.grid(
                            CONTROL_COLUMNS,
                            false,
                            CONTROL_KEYS.map(|(name, label, _)| (name, label)),
                        ),
                        Input::Full => screen.terminal_keys(&self.keys),
                    };
                }
                screen.build()
            }
        }
    }

    fn grid(&self, context: &Context) -> (u16, u16) {
        kobo_sdk::terminal_grid_for(
            &Self::screen_for_grid(self.grid_input),
            &context.metrics().oriented(Orientation::Landscape),
        )
    }
    fn screen_for_grid(input: Input) -> Screen {
        let screen = ScreenBuilder::new("paperterm-grid")
            .top_bar("Paperterm")
            .top_bar_action(REPAIR, "Pairing")
            .terminal(Vec::<String>::new(), None);
        match input {
            Input::None => screen,
            Input::Controls => screen.grid(
                CONTROL_COLUMNS,
                false,
                CONTROL_KEYS.map(|(name, label, _)| (name, label)),
            ),
            Input::Full => screen.terminal_keys(&TerminalKeys::new()),
        }
        .build()
    }
    fn hello(&mut self, context: &mut Context) {
        if self.poll.is_some() {
            return;
        }
        let (columns, rows) = self.grid(context);
        let url = format!(
            "https://{}/hello?token={}&grid={}x{}",
            self.address, self.code, columns, rows
        );
        self.poll = context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY_BYTES,
            credential: None,
            headers: Vec::new(),
        });
    }
    fn poll(&mut self, context: &mut Context) {
        if self.poll.is_some() || self.session.is_none() {
            return;
        }
        let url = format!(
            "https://{}/screen?token={}&session={}&seq={}&wait={LONGEST_POLL_SECONDS}",
            self.address,
            self.code,
            self.session.unwrap_or(0),
            self.sequence
        );
        self.poll = context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY_BYTES,
            credential: None,
            headers: Vec::new(),
        });
    }
    fn retry(&mut self, context: &mut Context) {
        self.nap = context.spawn(Task::Sleep {
            seconds: FAILURE_SLEEP_SECONDS,
        });
    }
    fn set_failure(&mut self, message: &str) -> bool {
        if self.failure.as_deref() == Some(message) {
            return false;
        }
        self.failure = Some(message.to_owned());
        true
    }
    fn clear_failure(&mut self) -> bool {
        self.failure.take().is_some()
    }
    fn save_pairing(&self, context: &mut Context) {
        context
            .store()
            .save(PAIRING, format!("{}\n{}", self.address, self.code));
    }
    fn send(&mut self, context: &mut Context, bytes: &[u8]) {
        let Some(session) = self.session else { return };
        if self.send.is_some() {
            return;
        }
        let body = format!(r#"{{"session":{session},"bytes_b64":"{}"}}"#, base64(bytes));
        self.send = context.spawn(Task::Post {
            url: format!("https://{}/keys?token={}", self.address, self.code),
            body,
            content_type: "application/json".to_owned(),
            credential: None,
            headers: Vec::new(),
            max_bytes: MAX_REPLY_BYTES,
        });
    }
    fn typed(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(pressed) = self.keyboard.press(action) else {
            return false;
        };
        if pressed == Pressed::Submitted {
            let text = self.keyboard.take();
            if self.view == View::Address {
                self.address = text;
                self.view = View::Code;
            } else {
                self.code = text;
                if self.code.len() == 6 {
                    self.save_pairing(context);
                    self.view = View::Watching;
                    self.hello(context);
                }
            }
        }
        self.show(context);
        true
    }
    fn parse_hello(&mut self, bytes: &[u8]) -> Hello {
        let Ok(value) = kobo_json::parse(std::str::from_utf8(bytes).unwrap_or("")) else {
            return Hello::Invalid;
        };
        let Some(session) = value
            .get("session")
            .and_then(kobo_json::Value::as_i64)
            .and_then(|id| u64::try_from(id).ok())
        else {
            return Hello::Invalid;
        };
        let input = Input::from_wire(
            value
                .get("input")
                .and_then(kobo_json::Value::as_str)
                .unwrap_or("none"),
        );
        self.input = input;
        if input != self.grid_input {
            self.grid_input = input;
            return Hello::Resize;
        }
        self.session = Some(session);
        self.sequence = 0;
        Hello::Ready
    }
    fn parse_screen(&mut self, bytes: &[u8]) -> Option<bool> {
        let Ok(value) = kobo_json::parse(std::str::from_utf8(bytes).unwrap_or("")) else {
            return None;
        };
        let sequence = value
            .get("seq")
            .and_then(kobo_json::Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())?;
        let rows = value.get("rows").and_then(kobo_json::Value::as_array)?;
        let mut changed = false;
        for row in rows {
            let (Some(y), Some(cells)) = (
                row.get("y")
                    .and_then(kobo_json::Value::as_i64)
                    .and_then(|value| usize::try_from(value).ok()),
                row.get("cells").and_then(kobo_json::Value::as_str),
            ) else {
                continue;
            };
            if self.rows.len() <= y {
                self.rows.resize(y + 1, String::new());
            }
            if self.rows[y] != cells {
                cells.clone_into(&mut self.rows[y]);
                changed = true;
            }
            let next_cursor = row
                .get("cursor")
                .and_then(kobo_json::Value::as_i64)
                .and_then(|value| u16::try_from(value).ok());
            if let Ok(row) = u16::try_from(y) {
                if let Some(column) = next_cursor {
                    let next = Some(Caret::new(row, column));
                    if self.cursor != next {
                        self.cursor = next;
                        changed = true;
                    }
                } else if self.cursor.is_some_and(|cursor| cursor.row == row) {
                    self.cursor = None;
                    changed = true;
                }
            }
        }
        self.sequence = sequence;
        if value.get("ended").and_then(kobo_json::Value::as_bool) == Some(true) {
            self.view = View::Ended;
            changed = true;
        }
        Some(changed)
    }
}

impl KoboApp for Paperterm {
    fn on_start(&mut self, context: &mut Context) {
        context.set_orientation(Orientation::Landscape);
        context.store().load(PAIRING);
        self.show(context);
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let StoreResult::Loaded { key, value } = result else {
            return;
        };
        if key != PAIRING {
            return;
        }
        if let Some((address, code)) = value
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| text.split_once('\n'))
        {
            address.trim().clone_into(&mut self.address);
            code.trim().clone_into(&mut self.code);
            self.view = View::Watching;
            self.hello(context);
        } else {
            self.view = View::Address;
        }
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if matches!(self.view, View::Address | View::Code) && self.typed(context, action) {
            return;
        }
        if action == action_id(REPAIR) {
            self.keyboard = Keyboard::with_text(&self.address);
            self.view = View::Address;
            self.session = None;
            self.show(context);
            return;
        }
        if matches!(self.view, View::Watching) {
            if let Some((_, _, bytes)) = CONTROL_KEYS
                .iter()
                .find(|(name, _, _)| action == action_id(name))
            {
                self.send(context, bytes);
                return;
            }
            if self.input == Input::Full {
                if let Some(typed) = self.keys.press(action) {
                    match typed {
                        Typed::Send(bytes) => self.send(context, &bytes),
                        Typed::Changed => self.show(context),
                    }
                }
            }
        }
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.poll == Some(task) {
            self.poll = None;
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if self.session.is_none() {
                        let previous_input = self.input;
                        match self.parse_hello(&bytes) {
                            Hello::Ready => {
                                if self.clear_failure() || self.input != previous_input {
                                    self.show(context);
                                }
                                if self.view != View::Ended {
                                    self.poll(context);
                                }
                            }
                            Hello::Resize => {
                                if self.clear_failure() || self.input != previous_input {
                                    self.show(context);
                                }
                                self.hello(context);
                            }
                            Hello::Invalid => {
                                if self.set_failure(PAIRING_REFUSED) {
                                    self.show(context);
                                }
                                self.retry(context);
                            }
                        }
                    } else if let Some(content_changed) = self.parse_screen(&bytes) {
                        let repaint = self.clear_failure() || content_changed;
                        if repaint {
                            self.show(context);
                        }
                        if self.view != View::Ended {
                            self.poll(context);
                        }
                    } else {
                        self.poll(context);
                    }
                }
                TaskOutcome::Failed(_) => {
                    self.session = None;
                    if self.set_failure(OFF_AIR) {
                        self.show(context);
                    }
                    self.retry(context);
                }
                TaskOutcome::Cancelled => {}
            }
        } else if self.nap == Some(task) {
            self.nap = None;
            if self.session.is_some() {
                self.poll(context);
            } else {
                self.hello(context);
            }
        } else if self.send == Some(task) {
            self.send = None;
            if !matches!(outcome, TaskOutcome::Completed(_)) && self.set_failure(INPUT_REFUSED) {
                self.show(context);
            }
        }
    }
    fn on_foreground(&mut self, context: &mut Context) {
        if self.view == View::Watching && self.poll.is_none() && self.nap.is_none() {
            self.poll(context);
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        text.push(A[((value >> 18) & 63) as usize] as char);
        text.push(A[((value >> 12) & 63) as usize] as char);
        text.push(if chunk.len() > 1 {
            A[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        text.push(if chunk.len() > 2 {
            A[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    text
}
fn main() -> ExitCode {
    kobo_sdk::run("paperterm", Paperterm::default()).map_or_else(
        |error| {
            eprintln!("paperterm: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, StoreRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn paired_start_measures_grid_and_asks_the_host() {
        let mut app = Paperterm::default();
        let mut context = Context::default();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRING.to_owned(),
                value: Some(b"host:9332\nabc123".to_vec()),
            },
        );
        assert!(context.commands().iter().any(|command| matches!(command, Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("/hello?token=abc123&grid="))));
    }
    #[test]
    fn changed_rows_repaint_but_an_empty_poll_does_not() {
        let mut app = Paperterm {
            session: Some(4),
            view: View::Watching,
            ..Paperterm::default()
        };
        assert_eq!(
            app.parse_screen(
                br#"{"seq":1,"rows":[{"y":0,"cells":"hello","cursor":5}],"ended":false,"exit":null}"#
            ),
            Some(true)
        );
        assert_eq!(
            app.parse_screen(br#"{"seq":1,"rows":[],"ended":false,"exit":null}"#),
            Some(false)
        );
    }
    #[test]
    fn controls_send_exact_bytes_and_full_input_is_not_shown_when_read_only() {
        assert_eq!(base64(b"\x1b[A"), "G1tB");
        let app = Paperterm {
            view: View::Watching,
            input: Input::None,
            rows: vec![String::new(); 20],
            ..Paperterm::default()
        };
        let screen = app.screen();
        assert!(screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(REPAIR))
            .is_some());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
    #[test]
    fn pairing_save_is_app_scoped() {
        let app = Paperterm {
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.save_pairing(&mut context);
        assert!(
            matches!(context.commands().first(), Some(Command::Store(StoreRequest::Save { key, .. })) if key == PAIRING)
        );
    }

    #[test]
    fn a_failed_poll_restarts_pairing_instead_of_retrying_a_dead_session() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            session: Some(4),
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert_eq!(app.session, None);
        assert_eq!(
            context
                .commands()
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count(),
            1
        );
        let nap = app.nap.expect("retry sleep");
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        assert!(context.commands().iter().any(
            |command| matches!(command, Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("/hello?"))
        ));
    }

    #[test]
    fn repeated_offline_failures_do_not_repaint() {
        let mut app = Paperterm {
            view: View::Watching,
            failure: Some(OFF_AIR.to_owned()),
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert!(!context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::SetScreen(_))));
        assert!(app.nap.is_some());
    }

    #[test]
    fn successful_hello_clears_offline_state_with_one_repaint() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            failure: Some(OFF_AIR.to_owned()),
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Completed(br#"{"session":4,"input":"full"}"#.to_vec()),
        );
        assert_eq!(app.failure, None);
        assert_eq!(
            context
                .commands()
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count(),
            1
        );
    }

    #[test]
    fn successful_resize_hello_clears_offline_state_before_renegotiating() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            failure: Some(OFF_AIR.to_owned()),
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Completed(br#"{"session":4,"input":"controls"}"#.to_vec()),
        );
        assert_eq!(app.failure, None);
        assert_eq!(app.session, None);
        assert_eq!(
            context
                .commands()
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count(),
            1
        );
        assert!(context.commands().iter().any(
            |command| matches!(command, Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("/hello?"))
        ));
    }

    #[test]
    fn unchanged_successful_hello_does_not_repaint() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            input: Input::Full,
            grid_input: Input::Full,
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Completed(br#"{"session":4,"input":"full"}"#.to_vec()),
        );
        assert!(!context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::SetScreen(_))));
    }

    #[test]
    fn successful_unchanged_screen_clears_failure_with_one_repaint() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            session: Some(4),
            failure: Some(OFF_AIR.to_owned()),
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Completed(br#"{"seq":0,"rows":[],"ended":false,"exit":null}"#.to_vec()),
        );
        assert_eq!(app.failure, None);
        assert_eq!(
            context
                .commands()
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count(),
            1
        );
    }

    #[test]
    fn cursor_only_deltas_move_and_clear_the_visible_caret() {
        let mut app = Paperterm {
            rows: vec!["prompt".to_owned()],
            cursor: Some(Caret::new(0, 3)),
            ..Paperterm::default()
        };
        assert_eq!(
            app.parse_screen(
                br#"{"seq":2,"rows":[{"y":0,"cells":"prompt","cursor":2}],"ended":false}"#
            ),
            Some(true)
        );
        assert_eq!(app.cursor, Some(Caret::new(0, 2)));
        assert_eq!(
            app.parse_screen(
                br#"{"seq":3,"rows":[{"y":0,"cells":"prompt","cursor":null}],"ended":false}"#
            ),
            Some(true)
        );
        assert_eq!(app.cursor, None);
    }

    #[test]
    fn paperterm_uses_the_terminal_viewport_reported_by_the_runtime() {
        let metrics = CLARA_BW_METRICS.oriented(Orientation::Landscape);
        let full = kobo_sdk::terminal_grid_for(&Paperterm::screen_for_grid(Input::Full), &metrics);
        let controls =
            kobo_sdk::terminal_grid_for(&Paperterm::screen_for_grid(Input::Controls), &metrics);
        assert!(full.0 > 0 && full.1 > 0);
        assert!(full.0 > full.1);
        assert_eq!(controls.0, full.0);
        assert!(
            controls.1 > full.1,
            "control mode {controls:?} should exceed full-keyboard mode {full:?}"
        );
    }

    #[test]
    fn paperterm_requests_landscape_before_its_first_screen() {
        let mut app = Paperterm::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        assert_eq!(
            context.commands().first(),
            Some(&Command::SetOrientation(Orientation::Landscape))
        );
    }

    #[test]
    fn terminal_rows_never_push_enabled_controls_off_panel() {
        for metrics in [
            CLARA_BW_METRICS,
            CLARA_BW_METRICS.oriented(Orientation::Landscape),
        ] {
            for input in [Input::Controls, Input::Full] {
                let app = Paperterm {
                    view: View::Watching,
                    input,
                    rows: vec!["terminal row".to_owned(); 64],
                    ..Paperterm::default()
                };
                let screen = app.screen();
                let chrome = Chrome::measuring(true);
                assert!(
                    screen.diagnostics(&metrics, &chrome).issues.is_empty(),
                    "{input:?} controls were clipped at {}x{}",
                    metrics.width,
                    metrics.height
                );
                let required = match input {
                    Input::Controls => CONTROL_KEYS
                        .iter()
                        .map(|(name, _, _)| action_id(name))
                        .collect::<Vec<_>>(),
                    Input::Full => ["term.esc", "term.right", "kb.r0c0", "kb.enter"]
                        .map(action_id)
                        .to_vec(),
                    Input::None => unreachable!(),
                };
                let layout = screen.layout_with(&metrics, &chrome);
                for action in required {
                    assert!(
                        layout.rect_of_action(action).is_some(),
                        "{input:?} action {action:?} was not visible at {}x{}",
                        metrics.width,
                        metrics.height
                    );
                }
            }
        }
    }

    #[test]
    fn negotiated_grid_matches_the_rows_the_layout_can_show() {
        let metrics = CLARA_BW_METRICS.oriented(Orientation::Landscape);
        for input in [Input::None, Input::Controls, Input::Full] {
            let (_, rows) =
                kobo_sdk::terminal_grid_for(&Paperterm::screen_for_grid(input), &metrics);
            let app = Paperterm {
                view: View::Watching,
                input,
                rows: vec!["terminal row".to_owned(); usize::from(rows)],
                ..Paperterm::default()
            };
            let layout = app.screen().layout_with(&metrics, &Chrome::measuring(true));
            let shown = layout
                .nodes
                .iter()
                .find(|node| node.kind == kobo_ui::LayoutKind::TerminalGrid)
                .expect("terminal")
                .text_lines
                .len();
            assert_eq!(shown, usize::from(rows), "{input:?}");
        }
    }

    #[test]
    fn hello_is_repeated_once_when_the_host_input_mode_changes_the_grid() {
        let reply = br#"{"session":42,"grid":"100x25","title":"claude","input":"controls"}"#;
        let mut app = Paperterm::default();
        assert_eq!(app.grid_input, Input::Full);
        assert_eq!(app.parse_hello(reply), Hello::Resize);
        assert_eq!(app.grid_input, Input::Controls);
        assert_eq!(app.input, Input::Controls);
        assert_eq!(app.session, None);
        assert_eq!(app.parse_hello(reply), Hello::Ready);
        assert_eq!(app.session, Some(42));
    }
}
