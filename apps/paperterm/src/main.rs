//! The device half of Paperterm. It renders host rows but never owns a shell.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::terminal::{TerminalKeys, Typed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Caret, Context, KoboApp, Screen, ScreenBuilder, Space,
    StoreResult, Task, TaskId, TaskOutcome,
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

struct Paperterm {
    view: View,
    keyboard: Keyboard,
    address: String,
    code: String,
    rows: Vec<String>,
    cursor: Option<Caret>,
    input: Input,
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
                            3,
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

    fn grid(context: &Context) -> (u16, u16) {
        kobo_sdk::terminal_grid_for(&Self::screen_for_grid(), &context.metrics())
    }
    fn screen_for_grid() -> Screen {
        ScreenBuilder::new("paperterm-grid")
            .top_bar("Paperterm")
            .top_bar_action(REPAIR, "Pairing")
            .terminal(Vec::<String>::new(), None)
            .terminal_keys(&TerminalKeys::new())
            .build()
    }
    fn hello(&mut self, context: &mut Context) {
        if self.poll.is_some() {
            return;
        }
        let (columns, rows) = Self::grid(context);
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
    fn parse_hello(&mut self, bytes: &[u8]) -> bool {
        let Ok(value) = kobo_json::parse(std::str::from_utf8(bytes).unwrap_or("")) else {
            return false;
        };
        let Some(session) = value
            .get("session")
            .and_then(kobo_json::Value::as_i64)
            .and_then(|id| u64::try_from(id).ok())
        else {
            return false;
        };
        self.session = Some(session);
        self.sequence = 0;
        self.input = Input::from_wire(
            value
                .get("input")
                .and_then(kobo_json::Value::as_str)
                .unwrap_or("none"),
        );
        true
    }
    fn parse_screen(&mut self, bytes: &[u8]) -> bool {
        let Ok(value) = kobo_json::parse(std::str::from_utf8(bytes).unwrap_or("")) else {
            return false;
        };
        let Some(sequence) = value
            .get("seq")
            .and_then(kobo_json::Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
        else {
            return false;
        };
        let Some(rows) = value.get("rows").and_then(kobo_json::Value::as_array) else {
            return false;
        };
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
            if let Some(column) = row
                .get("cursor")
                .and_then(kobo_json::Value::as_i64)
                .and_then(|value| u16::try_from(value).ok())
            {
                if let Ok(row) = u16::try_from(y) {
                    self.cursor = Some(Caret::new(row, column));
                }
            }
        }
        self.sequence = sequence;
        if value.get("ended").and_then(kobo_json::Value::as_bool) == Some(true) {
            self.view = View::Ended;
            changed = true;
        }
        changed
    }
}

impl KoboApp for Paperterm {
    fn on_start(&mut self, context: &mut Context) {
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
                    self.failure = None;
                    let repaint = if self.session.is_none() {
                        self.parse_hello(&bytes)
                    } else {
                        self.parse_screen(&bytes)
                    };
                    if self.session.is_none() {
                        self.failure =
                            Some("Pairing was refused — run kobo stream init.".to_owned());
                        self.retry(context);
                        self.show(context);
                    } else if self.view != View::Ended {
                        if repaint {
                            self.show(context);
                        }
                        self.poll(context);
                    }
                }
                TaskOutcome::Failed(_) => {
                    self.failure = Some("off the air".to_owned());
                    self.session = None;
                    self.show(context);
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
            if !matches!(outcome, TaskOutcome::Completed(_)) {
                self.failure = Some("Input was not accepted by your computer.".to_owned());
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
        assert!(app.parse_screen(
            br#"{"seq":1,"rows":[{"y":0,"cells":"hello","cursor":5}],"ended":false,"exit":null}"#
        ));
        assert!(!app.parse_screen(br#"{"seq":1,"rows":[],"ended":false,"exit":null}"#));
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
        let nap = app.nap.expect("retry sleep");
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        assert!(context.commands().iter().any(
            |command| matches!(command, Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("/hello?"))
        ));
    }

    #[test]
    fn paperterm_uses_the_terminal_viewport_reported_by_the_runtime() {
        let grid = kobo_sdk::terminal_grid_for(&Paperterm::screen_for_grid(), &CLARA_BW_METRICS);
        assert!(grid.0 > 0);
        assert!(grid.1 > 0);
    }
}
