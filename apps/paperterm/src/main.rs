//! The device half of Paperterm. It renders host rows but never owns a shell.
use kobo_sdk::keyboard::{Keyboard, Layer, Pressed};
use kobo_sdk::terminal::{TerminalKeys, Typed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Caret, Context, DisplayMetrics, KoboApp, Orientation, Screen,
    ScreenBuilder, Space, StoreResult, Task, TaskId, TaskOutcome,
};
use std::collections::VecDeque;
use std::process::ExitCode;

/// One host request may wait this long before returning an unchanged screen.
const LONGEST_POLL_SECONDS: u32 = 25;
/// Failed radio requests rest before trying again, with one request in flight.
const FAILURE_SLEEP_SECONDS: u32 = 5;
/// Bounded enough that a malformed host cannot fill the app task channel.
const MAX_REPLY_BYTES: u32 = 64 * 1024;
const MAX_KEY_BYTES: usize = 64;
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
    pairing_generation: u64,
    grid_generation: u64,
    hello: Option<TaskId>,
    hello_grid: Option<(u16, u16)>,
    hello_pairing_generation: u64,
    hello_grid_generation: u64,
    poll: Option<TaskId>,
    poll_pairing_generation: u64,
    poll_grid_generation: u64,
    nap: Option<TaskId>,
    nap_pairing_generation: u64,
    send: Option<TaskId>,
    send_pairing_generation: u64,
    send_queue: VecDeque<u8>,
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
            pairing_generation: 0,
            grid_generation: 0,
            hello: None,
            hello_grid: None,
            hello_pairing_generation: 0,
            hello_grid_generation: 0,
            poll: None,
            poll_pairing_generation: 0,
            poll_grid_generation: 0,
            nap: None,
            nap_pairing_generation: 0,
            send: None,
            send_pairing_generation: 0,
            send_queue: VecDeque::new(),
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
                screen = screen.terminal(self.rows.clone(), self.cursor);
                if let Some(failure) = &self.failure {
                    screen = screen.banner(BannerLevel::Attention, failure.clone());
                }
                if self.view == View::Ended {
                    screen = screen.secondary("The host held the final terminal screen.");
                } else {
                    screen = match self.input {
                        Input::Controls => screen.fill().grid(
                            CONTROL_COLUMNS,
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
        Self::grid_for(
            self.input,
            self.keyboard_open,
            &context.metrics().oriented(Orientation::Landscape),
        )
    }
    fn grid_for(input: Input, keyboard_open: bool, metrics: &DisplayMetrics) -> (u16, u16) {
        kobo_sdk::terminal_grid_for(&Self::screen_for_grid(input, keyboard_open), metrics)
    }
    fn screen_for_grid(input: Input, keyboard_open: bool) -> Screen {
        let mut screen = ScreenBuilder::new("paperterm-grid")
            .top_bar("Paperterm")
            .top_bar_action(REPAIR, "Pairing");
        if input == Input::Full {
            screen = screen.top_bar_action(
                TOGGLE_KEYBOARD,
                if keyboard_open {
                    "Close keys"
                } else {
                    "Keyboard"
                },
            );
        }
        screen = screen.terminal(Vec::<String>::new(), None);
        match (input, keyboard_open) {
            (Input::Controls, _) => screen.fill().grid(
                CONTROL_COLUMNS,
                false,
                CONTROL_KEYS.map(|(name, label, _)| (name, label)),
            ),
            (Input::Full, true) => paperterm_terminal_keys(screen, &TerminalKeys::new()),
            (Input::None | Input::Full, _) => screen,
        }
        .build()
    }
    fn retain_grid(&mut self, grid: (u16, u16)) {
        if self.viewport == grid {
            return;
        }
        self.rows.resize(usize::from(grid.1), String::new());
        self.viewport = grid;
    }
    fn hello(&mut self, context: &mut Context) {
        if self.hello.is_some() {
            return;
        }
        let (columns, rows) = self.grid(context);
        self.retain_grid((columns, rows));
        let url = format!(
            "https://{}/hello?token={}&grid={}x{}&generation={}",
            self.address, self.code, columns, rows, self.grid_generation
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
            self.hello_pairing_generation = self.pairing_generation;
            self.hello_grid_generation = self.grid_generation;
        }
    }
    fn renegotiate(&mut self, context: &mut Context) {
        self.grid_generation = self.grid_generation.saturating_add(1);
        self.retain_grid(self.grid(context));
        if let Some(task) = self.hello.take() {
            context.cancel(task);
        }
        self.hello_grid = None;
        if let Some(task) = self.poll.take() {
            context.cancel(task);
        }
        self.hello(context);
    }
    fn poll(&mut self, context: &mut Context) {
        if self.poll.is_some() || self.hello.is_some() || self.session.is_none() {
            return;
        }
        let url = format!(
            "https://{}/screen?token={}&session={}&seq={}&wait={LONGEST_POLL_SECONDS}&generation={}",
            self.address,
            self.code,
            self.session.unwrap_or(0),
            self.sequence,
            self.grid_generation
        );
        self.poll = context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY_BYTES,
            credential: None,
            headers: Vec::new(),
        });
        if self.poll.is_some() {
            self.poll_pairing_generation = self.pairing_generation;
            self.poll_grid_generation = self.grid_generation;
        }
    }
    fn retry(&mut self, context: &mut Context) {
        if self.nap.is_some() {
            return;
        }
        self.nap = context.spawn(Task::Sleep {
            seconds: FAILURE_SLEEP_SECONDS,
        });
        if self.nap.is_some() {
            self.nap_pairing_generation = self.pairing_generation;
        }
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
        if self.session.is_none() {
            return;
        }
        self.send_queue.extend(bytes);
        self.flush_send(context);
    }
    fn flush_send(&mut self, context: &mut Context) {
        let Some(session) = self.session else { return };
        if self.send.is_some() || self.send_queue.is_empty() {
            return;
        }
        let count = self.send_queue.len().min(MAX_KEY_BYTES);
        let bytes = self.send_queue.drain(..count).collect::<Vec<_>>();
        let body = format!(
            r#"{{"session":{session},"bytes_b64":"{}"}}"#,
            base64(&bytes)
        );
        self.send = context.spawn(Task::Post {
            url: format!("https://{}/keys?token={}", self.address, self.code),
            body,
            content_type: "application/json".to_owned(),
            credential: None,
            headers: Vec::new(),
            max_bytes: MAX_REPLY_BYTES,
        });
        if self.send.is_some() {
            self.send_pairing_generation = self.pairing_generation;
        } else {
            for byte in bytes.into_iter().rev() {
                self.send_queue.push_front(byte);
            }
            if self.set_failure(INPUT_REFUSED) {
                self.show(context);
            }
        }
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
        let input = Input::from_wire(
            value
                .get("input")
                .and_then(kobo_json::Value::as_str)
                .unwrap_or("none"),
        );
        self.input = input;
        if input != Input::Full {
            self.keyboard_open = false;
        }
        if self.session != Some(session) {
            self.sequence = 0;
        }
        self.session = Some(session);
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
            if self.viewport.1 != 0 && y >= usize::from(self.viewport.1) {
                continue;
            }
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
        if self.viewport.1 != 0 {
            self.rows
                .resize(usize::from(self.viewport.1), String::new());
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
    let mut lower = vec![(
        KB_SHIFT.to_string(),
        if shifted { "⇧•" } else { "⇧" }.to_string(),
    )];
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
            self.pairing_generation = self.pairing_generation.saturating_add(1);
            self.grid_generation = 0;
            if let Some(task) = self.hello.take() {
                context.cancel(task);
            }
            if let Some(task) = self.poll.take() {
                context.cancel(task);
            }
            if let Some(task) = self.nap.take() {
                context.cancel(task);
            }
            if let Some(task) = self.send.take() {
                context.cancel(task);
            }
            self.hello_grid = None;
            self.send_queue.clear();
            self.keyboard = Keyboard::with_text(&self.address);
            self.keyboard_open = false;
            self.input = Input::None;
            self.view = View::Address;
            self.session = None;
            self.show(context);
            return;
        }
        if matches!(self.view, View::Watching) {
            if action == action_id(TOGGLE_KEYBOARD) && self.input == Input::Full {
                self.keyboard_open = !self.keyboard_open;
                self.renegotiate(context);
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
            let current = self.hello_pairing_generation == self.pairing_generation
                && self.hello_grid_generation == self.grid_generation;
            self.hello = None;
            let requested_grid = self.hello_grid.take();
            if !current {
                return;
            }
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    let previous_input = self.input;
                    let previous_keyboard_open = self.keyboard_open;
                    if self.parse_hello(&bytes) {
                        let repaint = self.clear_failure()
                            || self.input != previous_input
                            || self.keyboard_open != previous_keyboard_open;
                        if repaint {
                            self.show(context);
                        }
                        if requested_grid == Some(self.grid(context)) {
                            if self.view != View::Ended {
                                self.poll(context);
                            }
                        } else {
                            self.grid_generation = self.grid_generation.saturating_add(1);
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
            let current = self.poll_pairing_generation == self.pairing_generation
                && self.poll_grid_generation == self.grid_generation;
            self.poll = None;
            if !current {
                return;
            }
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
            let current = self.nap_pairing_generation == self.pairing_generation;
            self.nap = None;
            if !current {
                return;
            }
            if self.session.is_some() {
                self.poll(context);
            } else {
                self.hello(context);
            }
        } else if self.send == Some(task) {
            let current = self.send_pairing_generation == self.pairing_generation;
            self.send = None;
            if !current {
                return;
            }
            if matches!(outcome, TaskOutcome::Completed(_)) {
                self.flush_send(context);
            } else if self.set_failure(INPUT_REFUSED) {
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
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    const HOST_MAX_COLUMNS: u16 = 300;
    const HOST_MAX_ROWS: u16 = 120;

    fn begin_hello(app: &mut Paperterm, context: &mut Context) -> TaskId {
        app.hello(context);
        app.hello.expect("hello task")
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

    fn assert_host_valid(grid: (u16, u16), context: &str) {
        assert!(
            (1..=HOST_MAX_COLUMNS).contains(&grid.0) && (1..=HOST_MAX_ROWS).contains(&grid.1),
            "{context}: {grid:?} is outside 1x1 through 300x120"
        );
    }

    fn wire_screen(screen: &kobo_stream::Screen) -> Vec<u8> {
        let rows = screen
            .rows
            .iter()
            .map(|row| {
                let cells = row.cells.replace('\\', "\\\\").replace('"', "\\\"");
                let cursor = row
                    .cursor
                    .map_or_else(|| "null".to_owned(), |column| column.to_string());
                format!(r#"{{"y":{},"cells":"{}","cursor":{cursor}}}"#, row.y, cells)
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"seq":{},"rows":[{rows}],"ended":{},"exit":null}}"#,
            screen.seq, screen.ended
        )
        .into_bytes()
    }

    fn occurrences(rows: &[String], needle: &str) -> usize {
        rows.iter().filter(|row| row.contains(needle)).count()
    }

    fn assert_retained_rows(actual: &[String], expected: &[String]) {
        assert!(actual.len() >= expected.len());
        assert_eq!(&actual[..expected.len()], expected);
        assert!(actual[expected.len()..].iter().all(String::is_empty));
    }

    fn posted_key_payloads(context: &Context) -> Vec<String> {
        context
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::Spawn {
                    work: Task::Post { body, url, .. },
                    ..
                } if url.contains("/keys?") => kobo_json::parse(body).ok().and_then(|value| {
                    value
                        .get("bytes_b64")
                        .and_then(kobo_json::Value::as_str)
                        .map(str::to_owned)
                }),
                _ => None,
            })
            .collect()
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
    fn initial_hidden_keyboard_hello_is_host_valid_and_reaches_stable_polling() {
        let mut app = Paperterm::default();
        let mut context = Context::default();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRING.to_owned(),
                value: Some(b"host:9332\nabc123".to_vec()),
            },
        );
        let url = context
            .commands()
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } if url.contains("/hello?") => Some(url),
                _ => None,
            })
            .expect("initial hello");
        assert_host_valid(grid_from_url(url), "Clara BW hidden-keyboard hello");

        let hello = app.hello.expect("hello task");
        app.on_task(
            &mut context,
            hello,
            TaskOutcome::Completed(br#"{"session":42,"input":"full"}"#.to_vec()),
        );
        assert_eq!(app.view, View::Watching);
        assert_eq!(app.session, Some(42));
        assert!(!app.keyboard_open);
        assert!(app.hello.is_none());
        assert!(
            app.poll.is_some(),
            "valid hello did not reach stable polling"
        );
    }

    #[test]
    fn every_profile_negotiates_host_valid_hidden_controls_and_keyboard_grids() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            let portrait = DisplayMetrics {
                width: i32::try_from(profile.width).expect("profile width"),
                height: i32::try_from(profile.height).expect("profile height"),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: kobo_ui::TextScale::Default,
            };
            let metrics = portrait.oriented(Orientation::Landscape);
            let hidden = Paperterm::grid_for(Input::Full, false, &metrics);
            let controls = Paperterm::grid_for(Input::Controls, false, &metrics);
            let open = Paperterm::grid_for(Input::Full, true, &metrics);
            for (state, grid) in [
                ("initial", Paperterm::grid_for(Input::None, false, &metrics)),
                ("hidden", hidden),
                ("controls", controls),
                ("open", open),
            ] {
                assert_host_valid(grid, &format!("{} {state}", profile.id));
            }
            assert_eq!(hidden.0, open.0, "{} columns", profile.id);
            assert!(hidden.1 > open.1, "{} keyboard rows", profile.id);
            assert!(hidden.1 > controls.1, "{} control rows", profile.id);
        }
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
        for metrics in [
            CLARA_BW_METRICS,
            CLARA_BW_METRICS.oriented(Orientation::Landscape),
        ] {
            let closed = Paperterm::grid_for(Input::Full, false, &metrics);
            let open = Paperterm::grid_for(Input::Full, true, &metrics);
            assert_eq!(closed.0, open.0);
            assert!(
                closed.1 > open.1,
                "{}x{} did not return the hidden keyboard space to the terminal",
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
    fn keyboard_toggle_renegotiates_without_replacing_terminal_state() {
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
        assert_retained_rows(&app.rows, &rows);
        assert_eq!(app.cursor, cursor);
        assert_eq!(app.session, Some(41));
        assert_eq!(app.sequence, 19);
        assert!(app.keys.is_control());
        let open_grid = app.hello_grid.expect("opening keys renegotiates");
        assert_host_valid(open_grid, "open keyboard");
        assert!(app
            .screen()
            .layout_with(
                &CLARA_BW_METRICS.oriented(Orientation::Landscape),
                &Chrome::measuring(true),
            )
            .rect_of_action(action_id("kb.r0c0"))
            .is_some());

        let hello = app.hello.expect("open-grid hello");
        app.on_task(
            &mut context,
            hello,
            TaskOutcome::Completed(br#"{"session":41,"input":"full"}"#.to_vec()),
        );
        assert_eq!(app.session, Some(41));
        assert_eq!(app.sequence, 19);
        let poll = app.poll.expect("poll resumes after resize");

        app.on_action(&mut context, action_id(TOGGLE_KEYBOARD));
        assert!(!app.keyboard_open);
        let close_hello = app.hello.expect("close-grid hello starts immediately");
        assert!(app.poll.is_none());
        assert!(context.commands().contains(&Command::Cancel(poll)));
        let closed_grid = app.hello_grid.expect("closing keys renegotiates");
        assert_host_valid(closed_grid, "closed keyboard");
        assert_eq!(closed_grid.0, open_grid.0);
        assert!(closed_grid.1 > open_grid.1);
        let retained = app.rows.clone();
        app.on_task(
            &mut context,
            poll,
            TaskOutcome::Completed(
                br#"{"seq":20,"rows":[{"y":0,"cells":"obsolete"}],"ended":false}"#.to_vec(),
            ),
        );
        assert_eq!(app.rows, retained, "obsolete poll changed resized rows");
        assert_eq!(app.hello, Some(close_hello));
        app.on_task(
            &mut context,
            close_hello,
            TaskOutcome::Completed(br#"{"session":41,"input":"full"}"#.to_vec()),
        );
        assert!(app.poll.is_some(), "resized polling did not resume");
        assert_retained_rows(&app.rows, &rows);
        assert_eq!(app.cursor, cursor);
        assert_eq!(app.session, Some(41));
        assert_eq!(app.sequence, 19);
        assert!(app.keys.is_control());
    }

    #[test]
    fn full_tui_hidden_open_closed_replaces_rows_without_a_stale_duplicate() {
        let metrics = CLARA_BW_METRICS.oriented(Orientation::Landscape);
        let hidden = Paperterm::grid_for(Input::Full, false, &metrics);
        let open = Paperterm::grid_for(Input::Full, true, &metrics);
        assert!(hidden.1 > open.1);
        let host = kobo_stream::Session::new(kobo_stream::Grid {
            columns: hidden.0,
            rows: hidden.1,
        });
        let mut tui = String::from("\x1b[2J\x1b[H");
        for _ in 0..hidden.1.saturating_sub(6) {
            tui.push_str("\r\n");
        }
        tui.push_str(
            "Quick safety check\r\n\
             Security guide\r\n\
             1. Yes, I trust this folder\r\n\
             2. No, exit\r\n\
             Enter to confirm",
        );
        host.feed(tui.as_bytes());

        let mut app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            session: Some(41),
            ..Paperterm::default()
        };
        app.retain_grid(hidden);
        assert_eq!(app.parse_screen(&wire_screen(&host.screen(0))), Some(true));
        assert_eq!(occurrences(&app.rows, "Enter to confirm"), 1);

        host.resize(kobo_stream::Grid {
            columns: open.0,
            rows: open.1,
        });
        host.feed(
            b"\x1b[2J\x1b[HQuick safety check\r\n\
              Security guide\r\n\
              1. Yes, I trust this folder\r\n\
              2. No, exit\r\n\
              Enter to confirm",
        );
        app.retain_grid(open);
        let smaller = host.screen(app.sequence);
        assert_eq!(app.parse_screen(&wire_screen(&smaller)), Some(true));
        assert_eq!(app.rows.len(), usize::from(open.1));
        assert_eq!(occurrences(&app.rows, "Enter to confirm"), 1);
        assert_eq!(app.session, Some(41));

        let cursor = app.cursor;
        app.retain_grid(hidden);
        assert_eq!(app.cursor, cursor, "grid expansion replaced the cursor");
        assert_eq!(app.session, Some(41));
        assert_eq!(occurrences(&app.rows, "Enter to confirm"), 1);
        assert!(app.rows[usize::from(open.1)..].iter().all(String::is_empty));

        host.resize(kobo_stream::Grid {
            columns: hidden.0,
            rows: hidden.1,
        });
        host.feed(
            b"\x1b[2J\x1b[HQuick safety check\r\n\
              Security guide\r\n\
              1. Yes, I trust this folder\r\n\
              2. No, exit\r\n\
              Enter to confirm",
        );
        let expanded = host.screen(app.sequence);
        assert!(app.parse_screen(&wire_screen(&expanded)).is_some());
        assert_eq!(app.rows.len(), usize::from(hidden.1));
        assert_eq!(occurrences(&app.rows, "Enter to confirm"), 1);
        assert!(app.rows[usize::from(open.1)..]
            .iter()
            .all(|row| row.trim().is_empty()));
        assert_eq!(app.session, Some(41));
    }

    #[test]
    fn rapid_keys_are_queued_in_order_and_batched_to_the_host_limit() {
        let mut app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            session: Some(41),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.send(&mut context, b"a");
        let first = app.send.expect("first key post");
        let queued = (0..130)
            .map(|index| b'b' + u8::try_from(index % 24).expect("letter"))
            .collect::<Vec<_>>();
        app.send(&mut context, &queued);
        assert_eq!(app.send_queue.len(), queued.len());

        app.on_task(&mut context, first, TaskOutcome::Completed(Vec::new()));
        let second = app.send.expect("first queued batch");
        app.on_task(&mut context, second, TaskOutcome::Completed(Vec::new()));
        let third = app.send.expect("second queued batch");
        app.on_task(&mut context, third, TaskOutcome::Completed(Vec::new()));
        let fourth = app.send.expect("final queued batch");
        app.on_task(&mut context, fourth, TaskOutcome::Completed(Vec::new()));

        let payloads = posted_key_payloads(&context);
        assert_eq!(
            payloads,
            vec![
                base64(b"a"),
                base64(&queued[..64]),
                base64(&queued[64..128]),
                base64(&queued[128..]),
            ]
        );
        assert!(app.send_queue.is_empty());
        assert!(app.send.is_none());
    }

    #[test]
    fn failed_key_post_is_visible_and_keeps_later_keys_queued() {
        let mut app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            session: Some(41),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.send(&mut context, b"a");
        let first = app.send.expect("active key post");
        app.send(&mut context, b"b");
        app.on_task(
            &mut context,
            first,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert_eq!(app.failure.as_deref(), Some(INPUT_REFUSED));
        assert_eq!(app.send_queue.iter().copied().collect::<Vec<_>>(), b"b");

        app.send(&mut context, b"c");
        assert!(app.send.is_some());
        assert_eq!(posted_key_payloads(&context).last(), Some(&base64(b"bc")));
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
    fn reconnect_retains_terminal_rows_and_cursor_then_resumes_polling() {
        let rows = vec!["Claude".to_owned(), "thinking…".to_owned()];
        let cursor = Some(Caret::new(1, 9));
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            rows: rows.clone(),
            cursor,
            input: Input::Full,
            session: Some(4),
            sequence: 27,
            poll: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert_retained_rows(&app.rows, &rows);
        assert_eq!(app.cursor, cursor);
        assert_eq!(app.failure.as_deref(), Some(OFF_AIR));

        let nap = app.nap.expect("retry sleep");
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        let hello = app.hello.expect("reconnect hello");
        app.on_task(
            &mut context,
            hello,
            TaskOutcome::Completed(br#"{"session":5,"input":"full"}"#.to_vec()),
        );
        assert_retained_rows(&app.rows, &rows);
        assert_eq!(app.cursor, cursor);
        assert_eq!(app.session, Some(5));
        assert_eq!(app.sequence, 0);
        assert_eq!(app.failure, None);
        assert!(app.poll.is_some());
    }

    #[test]
    fn repairing_cancels_retry_and_fences_every_old_host_outcome() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "old-host:9332".to_owned(),
            code: "old123".to_owned(),
            hello: Some(TaskId(9)),
            hello_grid: Some((80, 24)),
            poll: Some(TaskId(10)),
            nap: Some(TaskId(11)),
            send: Some(TaskId(12)),
            session: Some(4),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.on_action(&mut context, action_id(REPAIR));
        assert_eq!(app.pairing_generation, 1);
        assert!(app.hello.is_none());
        assert!(app.poll.is_none());
        assert!(app.nap.is_none());
        assert!(app.send.is_none());
        for task in [TaskId(9), TaskId(10), TaskId(11), TaskId(12)] {
            assert!(context.commands().contains(&Command::Cancel(task)));
        }

        app.address = "new-host:9332".to_owned();
        app.code = "new123".to_owned();
        app.view = View::Watching;
        app.hello(&mut context);
        let current = app.hello.expect("new pairing hello");
        assert!(context.commands().iter().any(
            |command| matches!(command, Command::Spawn { task, work: Task::Fetch { url, .. } }
                if *task == current && url.contains("new-host:9332/hello"))
        ));

        app.on_task(
            &mut context,
            TaskId(9),
            TaskOutcome::Completed(br#"{"session":99,"input":"full"}"#.to_vec()),
        );
        app.on_task(&mut context, TaskId(11), TaskOutcome::Completed(Vec::new()));
        assert_eq!(app.hello, Some(current));
        assert_eq!(app.session, None);
        assert_eq!(app.address, "new-host:9332");

        app.on_task(
            &mut context,
            current,
            TaskOutcome::Completed(br#"{"session":5,"input":"full"}"#.to_vec()),
        );
        assert_eq!(app.session, Some(5));
        assert!(app.poll.is_some());
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
    fn successful_resize_hello_clears_offline_state_before_renegotiating() {
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
            TaskOutcome::Completed(br#"{"session":4,"input":"controls"}"#.to_vec()),
        );
        assert_eq!(app.failure, None);
        assert_eq!(app.session, Some(4));
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
        assert!(app.hello.is_some());
    }

    #[test]
    fn unchanged_successful_hello_does_not_repaint() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            input: Input::Full,
            session: Some(4),
            hello: Some(TaskId(9)),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        app.hello_grid = Some(app.grid(&context));
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
        for metrics in [
            CLARA_BW_METRICS,
            CLARA_BW_METRICS.oriented(Orientation::Landscape),
        ] {
            let hidden = Paperterm::grid_for(Input::Full, false, &metrics);
            let open = Paperterm::grid_for(Input::Full, true, &metrics);
            let controls = Paperterm::grid_for(Input::Controls, false, &metrics);
            assert!(hidden.0 > 0 && hidden.1 > 0);
            assert_eq!(hidden.0, open.0);
            assert_eq!(controls.0, hidden.0);
            assert!(hidden.1 > open.1);
            assert!(hidden.1 > controls.1);
        }
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
            for (input, keyboard_open) in [
                (Input::Controls, false),
                (Input::Full, false),
                (Input::Full, true),
            ] {
                let app = Paperterm {
                    view: View::Watching,
                    input,
                    keyboard_open,
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
                let required = match (input, keyboard_open) {
                    (Input::Controls, _) => CONTROL_KEYS
                        .iter()
                        .map(|(name, _, _)| action_id(name))
                        .collect::<Vec<_>>(),
                    (Input::Full, true) => [
                        TOGGLE_KEYBOARD,
                        "term.esc",
                        "term.right",
                        "kb.r0c0",
                        KB_ENTER,
                    ]
                    .map(action_id)
                    .to_vec(),
                    (Input::Full, false) => vec![action_id(TOGGLE_KEYBOARD)],
                    (Input::None, _) => unreachable!(),
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
        for metrics in [
            CLARA_BW_METRICS,
            CLARA_BW_METRICS.oriented(Orientation::Landscape),
        ] {
            for (input, keyboard_open) in [
                (Input::None, false),
                (Input::Controls, false),
                (Input::Full, false),
                (Input::Full, true),
            ] {
                let (_, rows) = Paperterm::grid_for(input, keyboard_open, &metrics);
                let app = Paperterm {
                    view: View::Watching,
                    input,
                    keyboard_open,
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
                assert_eq!(
                    shown,
                    usize::from(rows),
                    "{input:?}, keyboard_open={keyboard_open}, {}x{}",
                    metrics.width,
                    metrics.height
                );
            }
        }
    }

    #[test]
    fn hello_is_repeated_once_when_the_host_input_mode_changes_the_grid() {
        let mut app = Paperterm {
            view: View::Watching,
            address: "host:9332".to_owned(),
            code: "abc123".to_owned(),
            ..Paperterm::default()
        };
        let mut context = Context::default();
        let first = begin_hello(&mut app, &mut context);
        let first_grid = app.hello_grid.expect("initial grid");
        assert_host_valid(first_grid, "initial hello");
        app.on_task(
            &mut context,
            first,
            TaskOutcome::Completed(
                br#"{"session":42,"grid":"100x25","title":"claude","input":"controls"}"#.to_vec(),
            ),
        );
        assert_eq!(app.input, Input::Controls);
        assert_eq!(app.session, Some(42));
        let second = app.hello.expect("control grid hello");
        let second_grid = app.hello_grid.expect("control grid");
        assert_host_valid(second_grid, "controls resize hello");
        assert_ne!(second_grid, first_grid);
        app.on_task(
            &mut context,
            second,
            TaskOutcome::Completed(br#"{"session":42,"input":"controls"}"#.to_vec()),
        );
        assert!(app.hello.is_none());
        assert!(app.poll.is_some());
    }

    #[test]
    fn landscape_keyboard_is_compact_and_all_keys_remain_in_bounds() {
        let metrics = CLARA_BW_METRICS.oriented(Orientation::Landscape);
        let app = Paperterm {
            view: View::Watching,
            input: Input::Full,
            keyboard_open: true,
            rows: vec!["terminal".to_owned(); 64],
            ..Paperterm::default()
        };
        let layout = app.screen().layout_with(&metrics, &Chrome::measuring(true));
        let keys = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, kobo_ui::LayoutKind::Cell(..)))
            .collect::<Vec<_>>();
        let mut key_rows = keys.iter().map(|key| key.rect.y).collect::<Vec<_>>();
        key_rows.sort_unstable();
        key_rows.dedup();
        assert_eq!(key_rows.len(), 4);
        assert!(keys.iter().all(|key| {
            key.rect.x >= 0
                && key.rect.y >= 0
                && key.rect.x + key.rect.width <= metrics.width
                && key.rect.y + key.rect.height <= metrics.height
                && key.rect.width >= metrics.touch_target_minimum()
                && key.rect.height >= metrics.touch_target_minimum()
        }));
    }
}
