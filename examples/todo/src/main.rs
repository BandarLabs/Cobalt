//! A list of things to do, which is where a platform's state model shows.
//!
//! It replaced a counter, because a counter demonstrates that a number can go
//! up and nothing else. This exercises the four things an application on this
//! device actually has to get right:
//!
//! - **State that outlives the process.** The list is written through
//!   [`kobo_sdk::AppStore`], so closing the application and opening it again
//!   shows the same list. Nothing here knows where that is stored, and there
//!   is no path it could name.
//! - **Actions that change one thing.** Tapping a row completes it. Only that
//!   row changes, so the runtime repaints that row rather than the screen,
//!   which on this panel is the difference between a flicker and a flash.
//! - **A state, drawn as the renderer sees fit.** A finished item is struck
//!   through and muted. The application never asks for a line through text; it
//!   says the item is done.
//! - **Typing, only where it is unavoidable.** Adding an item needs words, so
//!   the keyboard is raised for exactly that and put away again afterwards.
//!
//! ## Why the list is saved on every change
//!
//! There is no save button and no "are you sure". E Ink devices are closed by
//! shutting a cover and are forgotten until the battery is flat, so any design
//! that relies on a clean exit loses data. Each write is atomic, so the worst
//! a power loss can cost is the change that was in flight.

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, Context, KoboApp, LogLevel, Screen, ScreenBuilder, Space, StoreResult,
};
use std::process::ExitCode;

/// The one key this application uses.
const ITEMS: &str = "items";

/// How many items the list holds.
///
/// Not a storage limit: it is what fits on a panel that is turned rather than
/// scrolled, times a sensible number of pages. A list longer than this is a
/// different application.
const MAX_ITEMS: usize = 60;

/// How many items are shown at once.
const PER_PAGE: usize = 6;

const ADD: &str = "add";
const CLEAR_DONE: &str = "clear-done";
const PREVIOUS: &str = "previous";
const NEXT: &str = "next";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Item {
    text: String,
    done: bool,
}

/// The whole list, as bytes.
///
/// One item a line, prefixed by a single character saying whether it is done.
/// A newline can therefore never appear inside an item, which is enforced when
/// an item is added rather than escaped here: an escape scheme is a parser, and
/// a parser is a thing that can be wrong.
fn encode(items: &[Item]) -> Vec<u8> {
    let mut out = String::new();
    for item in items {
        out.push(if item.done { 'x' } else { '-' });
        out.push(' ');
        out.push_str(&item.text);
        out.push('\n');
    }
    out.into_bytes()
}

/// Reads the list back, skipping anything it does not recognise.
///
/// Deliberately forgiving in one direction only. A line this version cannot
/// read is dropped rather than refused, because refusing means an owner whose
/// state file was written by a newer build sees an empty list and no
/// explanation. Nothing here can be corrupted into something dangerous: the
/// worst case is a missing line.
fn decode(bytes: &[u8]) -> Vec<Item> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (flag, rest) = line.split_at_checked(2)?;
            let done = match flag {
                "x " => true,
                "- " => false,
                _ => return None,
            };
            if rest.is_empty() {
                return None;
            }
            Some(Item {
                text: rest.to_string(),
                done,
            })
        })
        .take(MAX_ITEMS)
        .collect()
}

struct Todo {
    items: Vec<Item>,
    /// Nothing is drawn as an empty list until the store has answered, because
    /// "you have nothing to do" and "your list has not arrived yet" are
    /// different statements and only one of them is reassuring.
    loaded: bool,
    page: usize,
    entry: TextEntry,
}

impl Default for Todo {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            loaded: false,
            page: 0,
            entry: TextEntry::new().opened_by(ADD),
        }
    }
}

impl Todo {
    fn show(&mut self, context: &mut Context) {
        let screen = if self.entry.is_open() {
            ScreenBuilder::new("Todo")
                .text_entry(&self.entry, "New item", "Add")
                .build()
        } else {
            self.list()
        };
        context.set_screen(screen);
    }

    fn list(&self) -> Screen {
        let mut screen = ScreenBuilder::new("Todo").top_bar(self.title());
        if !self.loaded {
            // The shape of the answer, before the answer. The list appears in
            // place of these lines rather than pushing them down, so nothing
            // moves under a finger that is already reaching.
            return screen.skeleton(4).build();
        }
        if self.items.is_empty() {
            screen = screen
                .heading("Nothing to do")
                .text("Tap Add to put something on the list.");
        } else {
            let start = self.page * PER_PAGE;
            screen = screen.checklist(
                self.items
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(PER_PAGE)
                    .map(|(index, item)| {
                        (
                            item_name(index),
                            item.text.clone(),
                            if item.done {
                                "Done. Tap to reopen.".to_string()
                            } else {
                                String::new()
                            },
                            item.done,
                        )
                    }),
            );
            if self.pages() > 1 {
                screen = screen.page_turns(PREVIOUS, NEXT);
            }
        }
        screen = screen.spacer(Space::Medium).button(ADD, "Add");
        if self.items.iter().any(|item| item.done) {
            screen = screen.button(CLEAR_DONE, "Clear finished");
        }
        screen.build()
    }

    fn title(&self) -> String {
        if !self.loaded {
            return "Todo".to_string();
        }
        let left = self.items.iter().filter(|item| !item.done).count();
        match left {
            0 if self.items.is_empty() => "Todo".to_string(),
            0 => "Todo: all done".to_string(),
            1 => "Todo: 1 left".to_string(),
            _ => format!("Todo: {left} left"),
        }
    }

    fn pages(&self) -> usize {
        self.items.len().div_ceil(PER_PAGE).max(1)
    }

    /// Writes the list back. Called after every change, never batched.
    fn save(&mut self, context: &mut Context) {
        let bytes = encode(&self.items);
        context.store().save(ITEMS, bytes);
    }

    /// Keeps the page in range after items are removed.
    fn clamp_page(&mut self) {
        self.page = self.page.min(self.pages() - 1);
    }

    fn toggle(&mut self, index: usize) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        item.done = !item.done;
        true
    }
}

fn item_name(index: usize) -> String {
    format!("item.{index}")
}

fn item_index(action: ActionId) -> Option<usize> {
    (0..MAX_ITEMS).find(|index| action_id(&item_name(*index)) == action)
}

impl KoboApp for Todo {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(ITEMS);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { value, .. } => {
                self.items = value.map(|bytes| decode(&bytes)).unwrap_or_default();
                self.loaded = true;
                self.clamp_page();
                self.show(context);
            }
            // A save that failed is worth saying out loud. Silently carrying on
            // means the reader believes a list that is not there.
            StoreResult::Denied(reason) => {
                self.loaded = true;
                context.log(
                    LogLevel::Warn,
                    format!("the list could not be saved: {reason}"),
                );
                self.show(context);
            }
            StoreResult::Saved { .. } | StoreResult::Forgotten { .. } | StoreResult::Keys(_) => {}
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The field first, because while it is open it owns the panel.
        if let Some(event) = self.entry.handle(action) {
            if let Typing::Submitted(text) = event {
                if self.items.len() < MAX_ITEMS {
                    self.items.push(Item { text, done: false });
                    // Land on the page the new item is on, or it looks as if
                    // nothing happened.
                    self.page = (self.items.len() - 1) / PER_PAGE;
                    self.save(context);
                }
            }
            self.show(context);
            return;
        }
        if let Some(index) = item_index(action) {
            if self.toggle(index) {
                self.save(context);
                self.show(context);
            }
            return;
        }
        match action {
            action if action == action_id(CLEAR_DONE) => {
                self.items.retain(|item| !item.done);
                self.clamp_page();
                self.save(context);
                self.show(context);
            }
            action if action == action_id(PREVIOUS) => {
                self.page = self.page.saturating_sub(1);
                self.show(context);
            }
            action if action == action_id(NEXT) => {
                self.page = (self.page + 1).min(self.pages() - 1);
                self.show(context);
            }
            _ => {}
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("todo", Todo::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("todo: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, item_name, Item, Todo, ADD, CLEAR_DONE, ITEMS, PER_PAGE};
    use kobo_sdk::{action_id, Command, Context, KoboApp, StoreRequest, StoreResult};
    use kobo_ui::LayoutKind;

    fn started(items: &[(&str, bool)]) -> Todo {
        let mut todo = Todo::default();
        let mut context = Context::default();
        todo.on_start(&mut context);
        let stored = encode(
            &items
                .iter()
                .map(|(text, done)| Item {
                    text: (*text).to_string(),
                    done: *done,
                })
                .collect::<Vec<_>>(),
        );
        todo.on_store(
            &mut context,
            StoreResult::Loaded {
                key: ITEMS.into(),
                value: Some(stored),
            },
        );
        todo
    }

    fn saved(commands: &[Command]) -> Option<Vec<u8>> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == ITEMS => {
                Some(value.clone())
            }
            _ => None,
        })
    }

    #[test]
    fn a_list_survives_being_written_and_read_back() {
        let items = vec![
            Item {
                text: "milk".into(),
                done: true,
            },
            Item {
                text: "a book with spaces and - dashes".into(),
                done: false,
            },
        ];
        assert_eq!(decode(&encode(&items)), items);
    }

    #[test]
    fn an_unreadable_line_costs_that_line_and_nothing_else() {
        let items = decode(b"- kept\nnonsense\nx also kept\n");
        assert_eq!(items.len(), 2);
        assert!(items[1].done);
    }

    #[test]
    fn the_first_run_shows_an_empty_list_rather_than_a_failure() {
        let mut todo = Todo::default();
        let mut context = Context::default();
        todo.on_start(&mut context);
        assert!(
            !todo.loaded,
            "the list was treated as empty before the store answered"
        );
        todo.on_store(
            &mut context,
            StoreResult::Loaded {
                key: ITEMS.into(),
                value: None,
            },
        );
        assert!(todo.loaded);
        assert!(todo.items.is_empty());
    }

    #[test]
    fn tapping_an_item_finishes_it_and_writes_the_list_immediately() {
        let mut todo = started(&[("milk", false)]);
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(&item_name(0)));
        let commands = context.take_commands();
        assert!(todo.items[0].done);
        assert_eq!(
            saved(&commands).as_deref(),
            Some(b"x milk\n".as_slice()),
            "the change was kept in memory only"
        );
    }

    #[test]
    fn a_finished_item_is_drawn_struck_through() {
        let todo = started(&[("milk", true), ("bread", false)]);
        let screen = todo.list();
        let struck = screen
            .layout()
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::RowTitleDone))
            .count();
        assert_eq!(struck, 1, "the wrong number of items were drawn finished");
    }

    #[test]
    fn adding_an_item_needs_the_keyboard_and_the_row_opens_it() {
        let mut todo = started(&[]);
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(ADD));
        assert!(
            todo.entry.is_open(),
            "tapping add did not raise the keyboard"
        );
        for name in ["kb.r0c0", "kb.r0c1"] {
            todo.on_action(&mut context, action_id(name));
        }
        let _ignored = context.take_commands();
        todo.on_action(&mut context, action_id("kb.enter"));
        assert!(!todo.entry.is_open(), "the keyboard stayed up after adding");
        assert_eq!(todo.items.len(), 1);
        assert_eq!(todo.items[0].text, "qw");
        assert_eq!(
            saved(&context.take_commands()).as_deref(),
            Some(b"- qw\n".as_slice())
        );
    }

    #[test]
    fn backing_out_of_the_keyboard_adds_nothing() {
        let mut todo = started(&[]);
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(ADD));
        todo.on_action(&mut context, action_id("kb.r0c0"));
        todo.on_action(&mut context, action_id("kb.cancel"));
        assert!(!todo.entry.is_open());
        assert!(todo.items.is_empty());
    }

    #[test]
    fn clearing_finished_items_keeps_the_page_in_range() {
        let mut items: Vec<(&str, bool)> = Vec::new();
        for _ in 0..(PER_PAGE * 2) {
            items.push(("done", true));
        }
        items.push(("left", false));
        let mut todo = started(&items);
        todo.page = 2;
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(CLEAR_DONE));
        assert_eq!(todo.items.len(), 1);
        assert_eq!(todo.page, 0, "the list showed a page that no longer exists");
    }

    #[test]
    fn a_refused_save_is_reported_rather_than_swallowed() {
        let mut todo = started(&[]);
        let mut context = Context::default();
        todo.on_store(
            &mut context,
            StoreResult::Denied(kobo_sdk::StoreError::Unwritable),
        );
        assert!(
            context
                .take_commands()
                .iter()
                .any(|command| matches!(command, Command::Log { .. })),
            "a failed save said nothing"
        );
    }
}
