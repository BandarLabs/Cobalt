//! The device half of Paperterm. It renders host rows but never owns a shell.
use kobo_sdk::keyboard::{Keyboard, Layer, Pressed};
use kobo_sdk::terminal::{TerminalKeys, Typed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Caret, Chrome, Context, DisplayMetrics, KoboApp, Screen,
    ScreenBuilder, Space, StoreResult, Task, TaskId, TaskOutcome,
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
const TOGGLE_KEYBOARD: &str = "toggle-keyboard";
const OFF_AIR: &str = "off the air";
const PAIRING_REFUSED: &str = "Pairing was refused — run kobo stream init.";
const INPUT_REFUSED: &str = "Input was not accepted by your computer.";
const KB_SHIFT: &str = "kb.shift";
const KB_LAYER: &str = "kb.layer";
const KB_SPACE: &str = "kb.space";
const KB_BACKSPACE: &str = "kb.backspace";
const KB_ENTER: &str = "kb.enter";
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
    keyboard_open: bool,
    viewport: (u16, u16),
    session: Option<u64>,
    sequence: u64,
    hello: Option<TaskId>,
    hello_grid: Option<(u16, u16)>,
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
            keyboard_open: false,
            viewport: (0, 0),
            session: None,
            sequence: 0,
            hello: None,
            hello_grid: None,
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
                .top_bar_action(REPAIR, "Pairing");
                if self.view == View::Watching && self.input == Input::Full {
                    screen = screen.top_bar_action(
                        TOGGLE_KEYBOARD,
                        if self.keyboard_open {
                            "Close keys"
                        } else {
                            "Keyboard"
                        },
                    );
                }
                let visible_rows = if self.viewport.1 == 0 {
                    self.rows.clone()
                } else {
                    self.rows
                        .iter()
                        .take(usize::from(self.viewport.1))
                        .cloned()
                        .collect()
                };
                let visible_cursor = self.cursor.filter(|cursor| {
                    self.viewport == (0, 0)
                        || (cursor.row < self.viewport.1 && cursor.column < self.viewport.0)
                });
                screen = screen.terminal(visible_rows, visible_cursor);
                if let Some(failure) = &self.failure {
                    screen = screen.banner(BannerLevel::Attention, failure.clone());
                }
                if self.view == View::Ended {
                    screen = screen.secondary("The host held the final terminal screen.");
                } else {
                    screen = match self.input {
                        Input::Controls => screen.grid(
                            3,
                            false,
                            CONTROL_KEYS.map(|(name, label, _)| (name, label)),
                        ),
                        Input::Full if self.keyboard_open => {
                            paperterm_terminal_keys(screen, &self.keys)
                        }
                        Input::None | Input::Full => screen,
                    };
                }
                screen.build()
            }
        }
    }

    fn grid(&self, context: &Context) -> (u16, u16) {
        Self::grid_for(self.input, self.keyboard_open, &context.metrics())
    }
    fn grid_for(input: Input, keyboard_open: bool, metrics: &DisplayMetrics) -> (u16, u16) {
        let empty = Self::screen_for_grid(input, keyboard_open, Vec::new());
        let (columns, mut rows) = kobo_sdk::terminal_grid_for(&empty, metrics);
        while rows > 0 {
            let full = Self::screen_for_grid(
                input,
                keyboard_open,
                vec!["x".repeat(usize::from(columns)); usize::from(rows)],
            );
            let layout = full.layout_with(metrics, &Chrome::measuring(true));
            let controls_fit = match input {
                Input::Controls => layout.rect_of_action(action_id("key-ctrl-c")).is_some(),
                Input::Full if keyboard_open => {
                    layout.rect_of_action(action_id(KB_ENTER)).is_some()
                }
                Input::None | Input::Full => true,
            };
            let bottom = layout
                .nodes
                .iter()
                .map(|node| node.rect.y + node.rect.height)
                .max()
                .unwrap_or(0);
            if controls_fit && bottom <= metrics.height {
                break;
            }
            rows -= 1;
        }
        (columns, rows)
    }
    fn screen_for_grid(input: Input, keyboard_open: bool, rows: Vec<String>) -> Screen {
        let mut screen = ScreenBuilder::new("paperterm-grid")
            .top_bar("Paperterm")
            .top_bar_action(REPAIR, "Pairing")
            .terminal(rows, None);
        screen = match input {
            Input::Controls => {
                screen.grid(3, false, CONTROL_KEYS.map(|(name, label, _)| (name, label)))
            }
            Input::Full if keyboard_open => paperterm_terminal_keys(screen, &TerminalKeys::new()),
            Input::None | Input::Full => screen,
        };
        screen.build()
    }
    fn hello(&mut self, context: &mut Context) {
        if self.hello.is_some() {
            return;
        }
        let (columns, rows) = self.grid(context);
        self.viewport = (columns, rows);
        let url = format!(
            "https://{}/hello?token={}&grid={}x{}",
            self.address, self.code, columns, rows
        );
        self.hello = context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY_BYTES,
            credential: None,
            headers: Vec::new(),
        });
        if self.hello.is_some() {
            self.hello_grid = Some((columns, rows));
        }
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
        if self.session != Some(session) {
            self.sequence = 0;
        }
        self.session = Some(session);
        self.input = Input::from_wire(
            value
                .get("input")
                .and_then(kobo_json::Value::as_str)
                .unwrap_or("none"),
        );
        if self.input != Input::Full {
            self.keyboard_open = false;
        }
        true
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

fn paperterm_terminal_keys(screen: ScreenBuilder, keys: &TerminalKeys) -> ScreenBuilder {
    let keyboard = keys.keyboard();
    let shifted = keyboard.is_shifted();
    let row = |row, characters: &str| {
        characters
            .chars()
            .enumerate()
            .map(|(column, character)| {
                (
                    format!("kb.r{row}c{column}"),
                    if shifted {
                        character.to_ascii_uppercase().to_string()
                    } else {
                        character.to_string()
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let rows = match keyboard.layer() {
        Layer::Letters => ["qwertyuiop", "asdfghjkl", "zxcvbnm"],
        Layer::Symbols => ["1234567890", "-/:;()&@\"", ".,?!'+="],
    };
    let top = row(0, rows[0]);
    let mut home = row(1, rows[1]);
    home.push((
        "term.ctrl".to_string(),
        if keys.is_control() { "CTRL" } else { "ctrl" }.to_string(),
    ));
    let mut lower = Vec::new();
    lower.push((
        KB_SHIFT.to_string(),
        if shifted { "⇧•" } else { "⇧" }.to_string(),
    ));
    lower.extend(row(2, rows[2]));
    lower.push((KB_BACKSPACE.to_string(), "back".to_string()));
    lower.push(("term.esc".to_string(), "esc".to_string()));

    screen
        .fill()
        .grid(u8::try_from(top.len()).unwrap_or(u8::MAX), false, top)
        .grid(u8::try_from(home.len()).unwrap_or(u8::MAX), false, home)
        .grid(u8::try_from(lower.len()).unwrap_or(u8::MAX), false, lower)
        .grid(
            8,
            false,
            [
                ("term.tab".to_string(), "tab".to_string()),
                (
                    KB_LAYER.to_string(),
                    match keyboard.layer() {
                        Layer::Letters => "?123".to_string(),
                        Layer::Symbols => "abc".to_string(),
                    },
                ),
                (KB_SPACE.to_string(), "space".to_string()),
                (KB_ENTER.to_string(), "enter".to_string()),
                ("term.up".to_string(), "up".to_string()),
                ("term.down".to_string(), "down".to_string()),
                ("term.left".to_string(), "left".to_string()),
                ("term.right".to_string(), "right".to_string()),
            ],
        )
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
            self.keyboard_open = false;
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
            self.keyboard_open = false;
            self.view = View::Address;
            self.session = None;
            self.show(context);
            return;
        }
        if matches!(self.view, View::Watching) {
            if action == action_id(TOGGLE_KEYBOARD) && self.input == Input::Full {
                self.keyboard_open = !self.keyboard_open;
                self.hello(context);
                self.show(context);
                return;
            }
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
        if self.hello == Some(task) {
            self.hello = None;
            let requested_grid = self.hello_grid.take();
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if self.parse_hello(&bytes) {
                        if requested_grid == Some(self.grid(context)) {
                            self.clear_failure();
                            self.show(context);
                            if self.view != View::Ended && self.poll.is_none() {
                                self.poll(context);
                            }
                        } else {
                            self.hello(context);
                        }
                    } else {
                        self.session = None;
                        if self.set_failure(PAIRING_REFUSED) {
                            self.show(context);
                        }
                        self.retry(context);
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
        } else if self.poll == Some(task) {
            self.poll = None;
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if let Some(content_changed) = self.parse_screen(&bytes) {
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
        if self.view == View::Watching
            && self.hello.is_none()
            && self.poll.is_none()
            && self.nap.is_none()
        {
            if self.session.is_some() {
                self.poll(context);
            } else {
                self.hello(context);
            }
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
    use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, CLARA_BW_METRICS};

    fn landscape() -> DisplayMetrics {
        DisplayMetrics {
            width: CLARA_BW_METRICS.height,
            height: CLARA_BW_METRICS.width,
            ..CLARA_BW_METRICS
        }
    }

    fn grid_from_url(url: &str) -> (u16, u16) {
        let grid = url
            .split("grid=")
            .nth(1)
            .expect("hello carries a grid")
            .split('&')
            .next()
            .expect("grid value");
        let (columns, rows) = grid.split_once('x').expect("COLSxROWS");
        (
            columns.parse().expect("numeric columns"),
            rows.parse().expect("numeric rows"),
        )
    }

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
    fn full_input_starts_with_the_keyboard_hidden_and_the_larger_canvas() {
        for metrics in [CLARA_BW_METRICS, landscape()] {
            let closed = Paperterm::grid_for(Input::Full, false, &metrics);
            let open = Paperterm::grid_for(Input::Full, true, &metrics);
            assert_eq!(closed.0, open.0);
            assert!(
                closed.1 > open.1,
                "{}x{} did not give the hidden keyboard space back to the terminal",
                metrics.width,
                metrics.height
            );

            let app = Paperterm {
                view: View::Watching,
                input: Input::Full,
                ..Paperterm::default()
            };
            let layout = app.screen().layout_with(&metrics, &Chrome::measuring(true));
            assert!(layout.rect_of_action(action_id(TOGGLE_KEYBOARD)).is_some());
            assert!(layout.rect_of_action(action_id("kb.r0c0")).is_none());
        }
    }

    #[test]
    fn keyboard_toggle_resizes_without_replacing_terminal_state() {
        let rows = vec!["first".to_owned(), "second".to_owned()];
        let cursor = Some(Caret::new(1, 3));
        let mut app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            rows: rows.clone(),
            cursor,
            session: Some(41),
            sequence: 19,
            ..Paperterm::default()
        };
        let _changed = app.keys.press(action_id("term.ctrl"));
        let mut context = Context::default();

        app.on_action(&mut context, action_id(TOGGLE_KEYBOARD));
        assert!(app.keyboard_open);
        assert_eq!(app.rows, rows);
        assert_eq!(app.cursor, cursor);
        assert_eq!(app.session, Some(41));
        assert_eq!(app.sequence, 19);
        assert!(app.keys.is_control());
        let opened_grid = app.hello_grid.expect("opening keys renegotiates the grid");
        let open_screen = context
            .commands()
            .iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("opening keys redraws");
        assert!(open_screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .rect_of_action(action_id("kb.r0c0"))
            .is_some());

        let hello = app.hello.expect("resize hello");
        app.on_task(
            &mut context,
            hello,
            TaskOutcome::Completed(br#"{"session":41,"input":"full"}"#.to_vec()),
        );
        assert_eq!(app.session, Some(41), "resize must retain the host session");

        app.on_action(&mut context, action_id(TOGGLE_KEYBOARD));
        assert!(!app.keyboard_open);
        let closed_grid = app.hello_grid.expect("closing keys renegotiates the grid");
        assert_eq!(closed_grid.0, opened_grid.0);
        assert!(closed_grid.1 > opened_grid.1);
        assert_eq!(app.rows, rows);
        assert_eq!(app.cursor, cursor);
        assert_eq!(app.sequence, 19);
        assert!(app.keys.is_control());
    }

    #[test]
    fn open_keyboard_and_top_bar_controls_stay_reachable_without_overlap() {
        for metrics in [CLARA_BW_METRICS, landscape()] {
            let grid = Paperterm::grid_for(Input::Full, true, &metrics);
            let app = Paperterm {
                view: View::Watching,
                input: Input::Full,
                keyboard_open: true,
                viewport: grid,
                rows: vec!["x".repeat(usize::from(grid.0)); usize::from(grid.1)],
                cursor: Some(Caret::new(grid.1.saturating_sub(1), 0)),
                ..Paperterm::default()
            };
            let screen = app.screen();
            let chrome = Chrome::measuring(true);
            assert!(
                screen.diagnostics(&metrics, &chrome).issues.is_empty(),
                "layout diagnostics failed at {}x{}: {:?}",
                metrics.width,
                metrics.height,
                screen.diagnostics(&metrics, &chrome).issues
            );
            let layout = screen.layout_with(&metrics, &chrome);
            let terminal = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::TerminalGrid)
                .expect("terminal");
            for action in [
                action_id(REPAIR),
                action_id(TOGGLE_KEYBOARD),
                action_id("term.esc"),
                action_id("term.right"),
                action_id("kb.r0c0"),
                action_id("kb.enter"),
            ] {
                let rect = layout
                    .rect_of_action(action)
                    .expect("control remains visible");
                assert!(rect.width >= metrics.touch_target_minimum());
                assert!(rect.height >= metrics.touch_target_minimum());
                assert!(rect.x >= 0 && rect.y >= 0);
                assert!(rect.x + rect.width <= metrics.width);
                assert!(rect.y + rect.height <= metrics.height);
                if action != action_id(REPAIR) && action != action_id(TOGGLE_KEYBOARD) {
                    assert!(terminal.rect.intersection(rect).is_none());
                }
            }

            let keys = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
                .collect::<Vec<_>>();
            for (index, key) in keys.iter().enumerate() {
                for other in &keys[index + 1..] {
                    assert!(key.rect.intersection(other.rect).is_none());
                }
            }
        }
    }

    #[test]
    fn landscape_uses_the_compact_four_row_terminal_keyboard() {
        let metrics = landscape();
        let viewport = Paperterm::grid_for(Input::Full, true, &metrics);
        let app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            keyboard_open: true,
            viewport,
            rows: vec!["terminal".to_owned(); 64],
            ..Paperterm::default()
        };
        let layout = app.screen().layout_with(&metrics, &Chrome::measuring(true));
        let keys = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
            .collect::<Vec<_>>();
        let mut rows = keys.iter().map(|key| key.rect.y).collect::<Vec<_>>();
        rows.sort_unstable();
        rows.dedup();
        assert_eq!(rows.len(), 4);
        assert!(keys
            .iter()
            .all(|key| key.rect.height >= metrics.touch_target_minimum()));
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
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.hello = Some(TaskId(9));
        app.hello_grid = Some(app.grid(&context));
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
    fn paperterm_uses_the_terminal_viewport_reported_by_the_runtime() {
        for metrics in [CLARA_BW_METRICS, landscape()] {
            for open in [false, true] {
                let grid = Paperterm::grid_for(Input::Full, open, &metrics);
                assert!(grid.0 > 0);
                assert!(grid.1 > 0);
            }
        }
    }

    #[test]
    fn hello_uses_the_grid_for_the_current_keyboard_state() {
        let mut app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.hello(&mut context);
        let url = context
            .commands()
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.as_str()),
                _ => None,
            })
            .expect("hello request");
        assert_eq!(grid_from_url(url), app.grid(&context));
    }
}
