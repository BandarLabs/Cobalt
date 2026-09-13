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
    action_id, ActionId, BandAlign, Context, ControlState, Glyph, KoboApp, LogLevel, Position,
    RowLead, Screen, ScreenBuilder, SlotWidth, Space, StoreResult,
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

const ADD: &str = "add";
const CLEAR_DONE: &str = "clear-done";
const UNDO_CLEAR: &str = "undo-clear";
const PREVIOUS: &str = "previous";
const NEXT: &str = "next";
/// What can be done to one item, on the screen that is about that item.
const EDIT: &str = "edit";
const MOVE_UP: &str = "move-up";
const MOVE_DOWN: &str = "move-down";
const DUE_TODAY: &str = "due-today";
const DUE_TOMORROW: &str = "due-tomorrow";
const DUE_WEEK: &str = "due-week";
const DUE_NONE: &str = "due-none";
const EXPORT: &str = "export";
const DELETE: &str = "delete";
/// The way into the screen where the list is changed rather than worked.
const EDIT_LIST: &str = "edit-list";
const EDIT_PREVIOUS: &str = "edit-previous";
const EDIT_NEXT: &str = "edit-next";

/// Seconds in a day, for turning the clock into a day number.
const DAY: i64 = 86_400;

/// Whether a control can act, said the way the toolkit says it.
const fn state_of(enabled: bool) -> ControlState {
    if enabled {
        ControlState::Enabled
    } else {
        ControlState::Disabled
    }
}

/// The action a row carries on the editing screen.
fn edit_name(index: usize) -> String {
    format!("edit.{index}")
}

/// Which item an editing row belongs to.
fn edit_index(action: ActionId) -> Option<usize> {
    (0..MAX_ITEMS).find(|index| action_id(&edit_name(*index)) == action)
}

/// Today, as days since the epoch.
///
/// A Kobo that has been asleep for a week comes back with whatever its clock
/// says, which may be wrong. Nothing here is worse for that than a date shown
/// a day out, which is why an overdue item is only ever drawn as overdue and
/// never acted on.
fn today() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or_default()
        / DAY
}

/// What a date says on the row it belongs to.
///
/// Relative, because a list of things to do is read against today and nothing
/// else: "in 3 days" is the answer to the question being asked and
/// "2026-09-15" is arithmetic homework.
fn when(due: i64, today: i64) -> String {
    match due - today {
        0 => "due today".to_owned(),
        1 => "due tomorrow".to_owned(),
        -1 => "a day late".to_owned(),
        days if days < 0 => format!("{} days late", -days),
        days if days <= 14 => format!("due in {days} days"),
        days => format!("due in {} weeks", days / 7),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Item {
    text: String,
    done: bool,
    /// When this is meant to be done by, if the owner said.
    ///
    /// Days since the epoch rather than a date, because the only questions
    /// asked of it are "is this before today" and "how many days apart are
    /// these", and both are subtraction. Optional, and optional is the point:
    /// a list where every entry demands a date is a list people stop using
    /// after a fortnight.
    due: Option<i64>,
}

/// The whole list, as bytes.
///
/// One item a line, prefixed by a single character saying whether it is done.
/// A newline can therefore never appear inside an item, which is enforced when
/// an item is added rather than escaped here: an escape scheme is a parser, and
/// a parser is a thing that can be wrong.
///
/// An item with a date carries it in a second field, `due=<day>`, between the
/// flag and the text. A list written by the version before dates reads back
/// unchanged, because a line with no date simply has no field: the format
/// grew rather than changed, so nobody's list has to be migrated.
fn encode(items: &[Item]) -> Vec<u8> {
    let mut out = String::new();
    for item in items {
        out.push(if item.done { 'x' } else { '-' });
        out.push(' ');
        if let Some(day) = item.due {
            out.push_str("due=");
            out.push_str(&day.to_string());
            out.push(' ');
        }
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
            // The date field, if there is one. Anything that is not a number
            // is not a date, and the line keeps its text rather than being
            // thrown away over it.
            let (due, rest) = match rest.strip_prefix("due=") {
                Some(tail) => match tail.split_once(' ') {
                    Some((day, text)) => (day.parse().ok(), text),
                    None => (None, rest),
                },
                None => (None, rest),
            };
            if rest.is_empty() {
                return None;
            }
            Some(Item {
                text: rest.to_string(),
                done,
                due,
            })
        })
        .take(MAX_ITEMS)
        .collect()
}

/// Which screen the owner is on.
///
/// Changing an item is a different job from working through the list, and it
/// is a rarer one. Keeping it on its own screen leaves the list as a list:
/// every row is one tap, and that tap means the one thing a list of things to
/// do is for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    List,
    /// The same list, drawn to be changed rather than ticked off.
    Editing,
    /// One item, by its index.
    Item(usize),
    /// A copy being prepared for the owner's computer.
    Saving,
}

struct Todo {
    items: Vec<Item>,
    /// Nothing is drawn as an empty list until the store has answered, because
    /// "you have nothing to do" and "your list has not arrived yet" are
    /// different statements and only one of them is reassuring.
    loaded: bool,
    page: usize,
    /// The tag the list is narrowed to, if any.
    ///
    /// A hashtag the owner wrote into an item, never a category this
    /// application invented: the only tags that exist are the ones already in
    /// the text, so nothing here can claim the list is organised in a way the
    /// owner did not organise it.
    filter: Option<String>,
    entry: TextEntry,
    /// Which screen is showing.
    view: View,
    /// The item whose words are being retyped, if any.
    ///
    /// The same keyboard adds an item and renames one; this is what says
    /// which of the two the words coming back belong to.
    renaming: Option<usize>,
    /// Which rows belong on each page, measured against this panel.
    ///
    /// A count used to be a constant. Six rows fit the panel it was written
    /// on at the size it was written at, and a list of long items at 170%
    /// drew four of them and lost the rest under the buttons.
    pages: Vec<Vec<usize>>,
    /// Which items a group heading stands above, by index.
    ///
    /// Kept rather than worked out while drawing, because the pagination was
    /// measured with these headings in these places: a screen that decides
    /// again where a heading goes draws one the measurement never counted,
    /// and the last row of that page goes through the buttons under it.
    headings: std::collections::BTreeMap<usize, &'static str>,
    /// The same, for the editing screen, which shows the list in the order it
    /// is kept rather than grouped by what is finished.
    edit_pages: Vec<Vec<usize>>,
    /// Which page of the editing screen is showing.
    edit_page: usize,
    /// What the last Clear finished took away, until the next change.
    ///
    /// One undo, kept in memory only. The alternative to this is asking "are
    /// you sure" before a button that is pressed once a week, which trains
    /// everybody to answer without reading it.
    cleared: Vec<Item>,
    /// Today, as days since the epoch, taken once per repaint.
    today: i64,
    /// The copy being prepared for a paired computer.
    export: Option<kobo_sdk::exports::Export>,
}

impl Default for Todo {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            loaded: false,
            page: 0,
            filter: None,
            entry: TextEntry::new().opened_by(ADD),
            view: View::default(),
            renaming: None,
            pages: Vec::new(),
            headings: std::collections::BTreeMap::new(),
            edit_pages: Vec::new(),
            edit_page: 0,
            cleared: Vec::new(),
            today: today(),
            export: None,
        }
    }
}

/// The tag a word carries, if it is one.
///
/// A leading `#` and at least one character after it. Trailing punctuation is
/// dropped so that "#milk." and "#milk" are the one tag rather than two, which
/// is the difference between a filter that groups the list and a filter that
/// splinters it.
fn tag_of(word: &str) -> Option<&str> {
    let tag = word
        .strip_prefix('#')?
        .trim_end_matches(|character: char| !character.is_alphanumeric());
    (!tag.is_empty()).then_some(tag)
}

/// Every tag written into one item, in the order they appear.
fn tags_of(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace().filter_map(tag_of)
}

fn tag_action(tag: &str) -> String {
    format!("tag.{tag}")
}

impl Todo {
    fn show(&mut self, context: &mut Context) {
        self.today = today();
        self.repaginate(&*context);
        let screen = if self.entry.is_open() {
            // The same keyboard for both jobs, saying which one it is doing.
            let (prompt, submit) = match self.renaming {
                Some(_) => ("Change the words", "Save"),
                None => ("New item", "Add"),
            };
            ScreenBuilder::new("Todo")
                .text_entry(&self.entry, prompt, submit)
                .build()
        } else {
            match self.view {
                View::List => self.list(),
                View::Editing => self.editing(),
                View::Item(index) => self.item(index),
                View::Saving => self
                    .export
                    .as_ref()
                    .map_or_else(|| self.list(), kobo_sdk::exports::Export::screen),
            }
        };
        context.set_screen(screen);
    }

    /// Which rows belong on each page of the list, measured against the panel
    /// this is actually running on.
    ///
    /// The two group headings are measured with the rows they introduce, so a
    /// page can never end with "Done" and nothing under it.
    fn repaginate(&mut self, context: &Context) {
        let order = self.ordered();
        self.headings.clear();
        for (at, &index) in order.iter().enumerate() {
            let item = &self.items[index];
            let first_done = item.done && (at == 0 || !self.items[order[at - 1]].done);
            if at == 0 && !item.done {
                self.headings.insert(index, "To do");
            } else if first_done {
                self.headings.insert(index, "Done");
            }
        }
        let rows: Vec<(Option<&str>, String, String)> = order
            .iter()
            .map(|&index| {
                let item = &self.items[index];
                (
                    self.headings.get(&index).copied(),
                    item.text.clone(),
                    self.summary(item),
                )
            })
            .collect();
        let borrowed: Vec<(Option<&str>, &str, &str)> = rows
            .iter()
            .map(|(section, title, summary)| (*section, title.as_str(), summary.as_str()))
            .collect();
        // Measured under everything the list is drawn around: the filter
        // chips above it and the buttons below. Both of those grow with the
        // reader's text setting, and a list measured without them puts its
        // last row through them at 170%.
        let pages = context.paginate_rows_in_sections_under(
            &borrowed,
            false,
            Position::AtTheFoot,
            &self.around_the_list(),
        );
        self.pages = pages
            .into_iter()
            .map(|page| page.into_iter().map(|at| order[at]).collect())
            .collect();
        if self.pages.is_empty() {
            self.pages.push(Vec::new());
        }
        self.page = self.page.min(self.pages.len() - 1);
        self.repaginate_editing(context);
    }

    /// The same, for the editing screen.
    ///
    /// Its own measurement because it draws a different list: every item, in
    /// the order they are kept, under a line of instructions and above two
    /// buttons. Sixty items is what this application holds, and sixty rows
    /// drawn in one go is fifty of them past the bottom of the panel.
    fn repaginate_editing(&mut self, context: &Context) {
        let rows: Vec<(String, String)> = self
            .items
            .iter()
            .map(|item| (item.text.clone(), self.summary(item)))
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        self.edit_pages = context.paginate_rows_under(
            &borrowed,
            false,
            Position::AtTheFoot,
            &Self::around_the_editing_list(),
        );
        if self.edit_pages.is_empty() {
            self.edit_pages.push(Vec::new());
        }
        self.edit_page = self.edit_page.min(self.edit_pages.len() - 1);
    }

    /// Everything the editing screen is drawn around.
    fn around_the_editing_list() -> Screen {
        ScreenBuilder::new("Todo")
            .secondary("Tap an item to rename it, move it, give it a date or remove it.")
            .spacer(Space::Medium)
            .buttons([(EXPORT, "Save a copy"), (ADD, "Add")])
            .build()
    }

    /// Everything the list is drawn around, built as a screen so it can be
    /// measured exactly as it will be drawn.
    ///
    /// The chips that narrow the list, and the buttons under it. Not the top
    /// bar: that is chrome the paginator already knows about.
    fn around_the_list(&self) -> Screen {
        let mut screen = ScreenBuilder::new("Todo");
        let tags = self.all_tags();
        if !tags.is_empty() {
            screen = screen.chips(tags.iter().map(|tag| {
                (
                    tag_action(tag),
                    format!("#{tag}"),
                    self.filter.as_deref() == Some(tag.as_str()),
                )
            }));
        }
        screen
            .spacer(Space::Medium)
            .buttons([(ADD, "Add"), (CLEAR_DONE, "Clear finished")])
            .build()
    }

    /// What a row says under its words: when it is due, or that it is finished.
    fn summary(&self, item: &Item) -> String {
        let due = item.due.map(|day| when(day, self.today));
        match (item.done, due) {
            // A finished item's date is behind it, whatever it was.
            (true, _) => "Done. Tap to reopen.".to_owned(),
            (false, Some(due)) => due,
            (false, None) => String::new(),
        }
    }

    fn list(&self) -> Screen {
        let mut screen = ScreenBuilder::new("Todo").top_bar(self.title());
        if !self.items.is_empty() {
            // Only where there is something to change. An Edit control over
            // an empty list is a promise about a screen with nothing on it.
            screen = screen.top_bar_action(EDIT_LIST, "Edit");
        }
        if !self.loaded {
            // The shape of the answer, before the answer. The list appears in
            // place of these lines rather than pushing them down, so nothing
            // moves under a finger that is already reaching.
            return screen.skeleton(4).build();
        }
        if self.items.is_empty() {
            // A splash rather than a heading and a paragraph: six words set
            // ranged left at the top of a 1448-pixel panel read as a page
            // that failed to load. The splash centres them in the room that
            // is left, and the Add button below still lands under them.
            screen = screen.splash(
                Some(Glyph::Check),
                "Nothing to do",
                "Tap Add to put something on the list.",
            );
        } else {
            let tags = self.all_tags();
            if !tags.is_empty() {
                screen = screen.chips(tags.iter().map(|tag| {
                    (
                        tag_action(tag),
                        format!("#{tag}"),
                        self.filter.as_deref() == Some(tag.as_str()),
                    )
                }));
            }
            let page = self.pages.get(self.page).cloned().unwrap_or_default();
            if page.is_empty() {
                screen = screen.text("Nothing on the list carries that tag.");
            } else {
                // Two labelled groups rather than one list, so "finished" is
                // said in a heading and not left to a line through the middle
                // of the text -- which a struck word alone leaves the reader to
                // squint at. The headings go where they were measured, and a
                // page that begins in the middle of a group carries none: the
                // struck words say which group that is.
                let mut run: Vec<usize> = Vec::new();
                for &index in &page {
                    if let Some(heading) = self.headings.get(&index) {
                        if !run.is_empty() {
                            screen = screen.checklist(run.drain(..).map(|index| self.row(index)));
                        }
                        screen = screen.section(*heading);
                    }
                    run.push(index);
                }
                if !run.is_empty() {
                    screen = screen.checklist(run.into_iter().map(|index| self.row(index)));
                }
                if self.pages.len() > 1 {
                    // The strip as well as the turns. A side tap that nothing
                    // on the panel mentions is a gesture nobody has: the list
                    // used to page silently, so a reader whose list had grown
                    // past one panel could not tell that it had.
                    screen = screen.page_turns(PREVIOUS, NEXT).page_position(
                        u16::try_from(self.page + 1).unwrap_or(1),
                        u16::try_from(self.pages.len()).unwrap_or(1),
                    );
                }
            }
        }
        screen = screen.spacer(Space::Medium);
        // Side by side, because stacked they were two full-width bars saying
        // one word each at the bottom of a list.
        if !self.cleared.is_empty() {
            // The way back from the one button here that throws something
            // away. Offered until the next change, and then gone: an undo
            // that outlives what it was undoing is a trap.
            screen = screen.buttons([(ADD, "Add"), (UNDO_CLEAR, "Undo clear")]);
        } else if self.items.iter().any(|item| item.done) {
            screen = screen.buttons([(ADD, "Add"), (CLEAR_DONE, "Clear finished")]);
        } else if self.items.is_empty() {
            // The one thing to do on an empty list, drawn as the one thing to
            // do. A plain button beside nothing at all reads as a footnote.
            screen = screen.primary_button(ADD, "Add a task");
        } else {
            screen = screen.button(ADD, "Add");
        }
        screen.build()
    }

    /// The same list, drawn to be changed.
    ///
    /// Every item, in the order they are kept rather than grouped by whether
    /// they are finished: this is the screen where that order can be moved,
    /// so it has to be the order that is shown.
    fn editing(&self) -> Screen {
        let mut screen = ScreenBuilder::new("Todo")
            .top_bar("Edit list")
            .owns_back(true)
            .secondary("Tap an item to rename it, move it, give it a date or remove it.");
        if self.items.is_empty() {
            screen = screen.empty_state("Nothing on the list yet.");
        } else {
            let page = self
                .edit_pages
                .get(self.edit_page)
                .cloned()
                .unwrap_or_default();
            let rows = page.iter().filter_map(|&index| {
                let item = self.items.get(index)?;
                Some((
                    edit_name(index),
                    item.text.clone(),
                    self.summary(item),
                    RowLead::from(if item.done { Glyph::Check } else { Glyph::Note }),
                ))
            });
            screen = screen.rows(rows);
            if self.edit_pages.len() > 1 {
                screen = screen.page_turns(EDIT_PREVIOUS, EDIT_NEXT).page_position(
                    u16::try_from(self.edit_page + 1).unwrap_or(1),
                    u16::try_from(self.edit_pages.len()).unwrap_or(1),
                );
            }
        }
        screen
            .spacer(Space::Medium)
            .buttons([(EXPORT, "Save a copy"), (ADD, "Add")])
            .build()
    }

    /// One item, and everything that can be done to it.
    fn item(&self, index: usize) -> Screen {
        let Some(item) = self.items.get(index) else {
            return self.list();
        };
        let last = self.items.len().saturating_sub(1);
        let screen = ScreenBuilder::new("Todo")
            .top_bar("Item")
            .owns_back(true)
            // The words themselves, set as text rather than as a heading. A
            // heading at the largest reader text setting is four lines of a
            // six inch panel for something the reader wrote and can already
            // read, and the controls that this screen exists for were pushed
            // off the bottom by it.
            .text(item.text.clone())
            .secondary(self.summary(item))
            .buttons([(EDIT, "Rename"), (DELETE, "Remove")]);
        // The pair stays a pair at the ends of the list, with whichever way
        // it cannot go drawn as unavailable. A control that disappears at the
        // edge of a list moves the other one under the finger reaching for it.
        let screen = if self.items.len() > 1 {
            let up = state_of(index > 0);
            let down = state_of(index < last);
            screen.section("Where it sits").band(
                BandAlign::Middle,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(move |screen: ScreenBuilder| {
                            screen.button_with_state(MOVE_UP, "Move up", up)
                        })
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        SlotWidth::Fill,
                        Box::new(move |screen: ScreenBuilder| {
                            screen.button_with_state(MOVE_DOWN, "Move down", down)
                        }),
                    ),
                ],
            )
        } else {
            screen
        };
        // Chips rather than a list of full-width choices. Four dates set as
        // four rows is most of a panel at the larger text settings, and the
        // screen they were on then had nowhere to put the last of them; four
        // short words sit on two lines and say the same thing. The one it
        // already has is drawn as chosen by the renderer rather than marked
        // in a label.
        let chosen = match item.due.map(|day| day - self.today) {
            Some(0) => Some(DUE_TODAY),
            Some(1) => Some(DUE_TOMORROW),
            Some(days) if days > 1 => Some(DUE_WEEK),
            Some(_) => None,
            None => Some(DUE_NONE),
        };
        screen
            .section("When it is due")
            .chips(
                [
                    (DUE_TODAY, "Today"),
                    (DUE_TOMORROW, "Tomorrow"),
                    (DUE_WEEK, "In a week"),
                    (DUE_NONE, "No date"),
                ]
                .into_iter()
                .map(|(name, label)| (name, label, chosen == Some(name))),
            )
            .build()
    }

    /// One checklist row: its action name, its text, what it says when done,
    /// and whether it is done.
    fn row(&self, index: usize) -> (String, String, String, bool) {
        let item = &self.items[index];
        (
            item_name(index),
            item.text.clone(),
            self.summary(item),
            item.done,
        )
    }

    /// Every distinct tag written into the list, in first-appearance order.
    ///
    /// First-appearance rather than alphabetical, because the owner sees the
    /// tags they typed in the order they typed them, and a list that quietly
    /// re-sorts what it was handed reads as if it knows better.
    fn all_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for item in &self.items {
            for tag in tags_of(&item.text) {
                if !tags.iter().any(|existing| existing.as_str() == tag) {
                    tags.push(tag.to_string());
                }
            }
        }
        tags
    }

    /// The indices to draw, filtered to the active tag and grouped so the
    /// unfinished come before the finished.
    ///
    /// A stable sort, so within each group the items keep the order they were
    /// added; the reader's list is not reshuffled every time one is ticked off.
    fn ordered(&self) -> Vec<usize> {
        let mut order: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| self.shown(item))
            .map(|(index, _)| index)
            .collect();
        order.sort_by_key(|&index| self.items[index].done);
        order
    }

    fn shown(&self, item: &Item) -> bool {
        match &self.filter {
            None => true,
            Some(tag) => tags_of(&item.text).any(|candidate| candidate == tag),
        }
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

    /// Writes the list back. Called after every change, never batched.
    fn save(&mut self, context: &mut Context) {
        let bytes = encode(&self.items);
        context.store().save(ITEMS, bytes);
    }

    /// Keeps the page in range after items are removed, and drops a filter for
    /// a tag that no longer exists.
    ///
    /// Clearing finished items can take the last item carrying a tag with it.
    /// Left set, that filter would show an empty list with no chip to switch it
    /// off -- a dead end the reader did not ask for.
    fn clamp_page(&mut self) {
        if let Some(tag) = &self.filter {
            if !self.all_tags().iter().any(|existing| existing == tag) {
                self.filter = None;
            }
        }
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }

    fn toggle(&mut self, index: usize) -> bool {
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        item.done = !item.done;
        true
    }

    /// The tag whose chip carries this action, if any.
    fn tapped_tag(&self, action: ActionId) -> Option<String> {
        self.all_tags()
            .into_iter()
            .find(|tag| action_id(&tag_action(tag)) == action)
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

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(export) = self.export.as_mut() {
            if export.on_save(context, key, &result) {
                self.show(context);
            }
        }
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if let Some(export) = self.export.as_mut() {
            if export.on_shelf(context, name, &result) {
                self.show(context);
            }
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        // The copy being prepared for a computer travels through the same
        // store as the list, under its own keys. Answering every load with
        // "this is the list" is how saving a copy emptied one.
        if let Some(export) = self.export.as_mut() {
            let key = match &result {
                StoreResult::Loaded { key, .. } | StoreResult::Saved { key } => key.clone(),
                _ => String::new(),
            };
            if key != ITEMS && export.on_save(context, &key, &result) {
                self.show(context);
                return;
            }
        }
        match result {
            StoreResult::Loaded { key, value } if key == ITEMS => {
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
            // Listed rather than wildcarded, so adding a store answer to the
            // protocol makes every application decide what it means here.
            // This one keeps nothing on the shelf.
            // A load for anything else belongs to something that has already
            // been offered it, and is not the list.
            StoreResult::Loaded { .. }
            | StoreResult::Saved { .. }
            | StoreResult::Forgotten { .. }
            | StoreResult::Keys(_)
            | StoreResult::ShelfWritten { .. }
            | StoreResult::ShelfRead { .. }
            | StoreResult::ShelfRemoved { .. }
            | StoreResult::Shelf(_) => {}
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The field first, because while it is open it owns the panel.
        if let Some(event) = self.entry.handle(action) {
            if let Typing::Submitted(text) = event {
                if let Some(index) = self.renaming.take() {
                    // The same words in the same place: renaming an item does
                    // not move it, and a list that reshuffles itself when a
                    // typo is fixed is a list nobody trusts.
                    if let Some(item) = self.items.get_mut(index) {
                        item.text = text;
                        self.cleared.clear();
                        self.save(context);
                    }
                    self.show(context);
                    return;
                }
                if self.items.len() < MAX_ITEMS {
                    self.items.push(Item {
                        text,
                        done: false,
                        due: None,
                    });
                    // A new item is unfinished, so it may sort above the page
                    // the reader was on. Clear any filter and land on wherever
                    // it actually landed, or it looks as if nothing happened.
                    self.filter = None;
                    let added = self.items.len() - 1;
                    self.repaginate(&*context);
                    // Onto the page it actually landed on, which depends on
                    // how many rows this panel holds at this text size, so it
                    // is looked up rather than divided out.
                    self.page = self
                        .pages
                        .iter()
                        .position(|page| page.contains(&added))
                        .unwrap_or(0);
                    self.save(context);
                }
            }
            self.show(context);
            return;
        }
        if let Some(tag) = self.tapped_tag(action) {
            self.filter = if self.filter.as_deref() == Some(tag.as_str()) {
                None
            } else {
                Some(tag)
            };
            self.page = 0;
            self.show(context);
            return;
        }
        if self.editing_action(context, action) {
            return;
        }
        if let Some(index) = item_index(action) {
            if self.toggle(index) {
                self.cleared.clear();
                self.save(context);
                self.show(context);
            }
            return;
        }
        match action {
            action if action == action_id(CLEAR_DONE) => {
                // Kept, rather than gone: one undo, until the next change.
                self.cleared = self
                    .items
                    .iter()
                    .filter(|item| item.done)
                    .cloned()
                    .collect();
                self.items.retain(|item| !item.done);
                self.clamp_page();
                self.save(context);
                self.show(context);
            }
            action if action == action_id(UNDO_CLEAR) => {
                let restored = std::mem::take(&mut self.cleared);
                for item in restored {
                    if self.items.len() < MAX_ITEMS {
                        self.items.push(item);
                    }
                }
                self.save(context);
                self.show(context);
            }
            action if action == action_id(PREVIOUS) => {
                self.page = self.page.saturating_sub(1);
                self.show(context);
            }
            action if action == action_id(NEXT) => {
                self.page = (self.page + 1).min(self.pages.len().saturating_sub(1));
                self.show(context);
            }
            _ => {}
        }
    }
}

impl Todo {
    /// The taps that belong to editing: the screen itself, one item, and
    /// everything that can be done to that item.
    ///
    /// Returns whether it took the tap.
    fn editing_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == ActionId::BACK {
            // One step at a time: an item goes back to the editing list, and
            // the editing list goes back to the list itself.
            self.view = match self.view {
                View::Item(_) | View::Saving => View::Editing,
                _ => View::List,
            };
            if self.view == View::List {
                self.export = None;
            }
            self.show(context);
            return true;
        }
        if action == action_id(EDIT_LIST) {
            self.view = View::Editing;
            self.edit_page = 0;
            self.show(context);
            return true;
        }
        if action == action_id(EDIT_NEXT) {
            self.edit_page = (self.edit_page + 1).min(self.edit_pages.len().saturating_sub(1));
            self.show(context);
            return true;
        }
        if action == action_id(EDIT_PREVIOUS) {
            self.edit_page = self.edit_page.saturating_sub(1);
            self.show(context);
            return true;
        }
        if let Some(index) = edit_index(action) {
            self.view = View::Item(index);
            self.show(context);
            return true;
        }
        let View::Item(index) = self.view else {
            return self.export_action(context, action);
        };
        if action == action_id(EDIT) {
            self.renaming = Some(index);
            let text = self.items.get(index).map(|item| item.text.clone());
            self.entry.open_with(text.unwrap_or_default());
            self.show(context);
            return true;
        }
        if action == action_id(DELETE) {
            if index < self.items.len() {
                self.items.remove(index);
                self.cleared.clear();
                self.save(context);
            }
            self.view = View::Editing;
            self.show(context);
            return true;
        }
        if action == action_id(MOVE_UP) || action == action_id(MOVE_DOWN) {
            let up = action == action_id(MOVE_UP);
            let swap = if up {
                index.checked_sub(1)
            } else {
                Some(index + 1).filter(|next| *next < self.items.len())
            };
            if let Some(swap) = swap {
                self.items.swap(index, swap);
                self.view = View::Item(swap);
                self.cleared.clear();
                self.save(context);
            }
            self.show(context);
            return true;
        }
        for (name, days) in [
            (DUE_TODAY, Some(0)),
            (DUE_TOMORROW, Some(1)),
            (DUE_WEEK, Some(7)),
            (DUE_NONE, None),
        ] {
            if action == action_id(name) {
                if let Some(item) = self.items.get_mut(index) {
                    item.due = days.map(|days| self.today + days);
                    self.cleared.clear();
                    self.save(context);
                }
                self.show(context);
                return true;
            }
        }
        false
    }

    /// The copy offered to a paired computer.
    ///
    /// The whole list as plain text, which is what a list of things to do is:
    /// nothing here is worth a format that needs a program to read it.
    fn export_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == action_id(EXPORT) {
            let mut text = String::new();
            for item in &self.items {
                text.push_str(if item.done { "[x] " } else { "[ ] " });
                text.push_str(&item.text);
                if let Some(day) = item.due {
                    text.push_str(" (");
                    text.push_str(&when(day, self.today));
                    text.push(')');
                }
                text.push('\n');
            }
            match kobo_sdk::exports::Export::new(
                "Todo",
                kobo_sdk::exports::Format::Text,
                text.into_bytes(),
            ) {
                Ok(mut export) => {
                    export.begin(context);
                    self.export = Some(export);
                    self.view = View::Saving;
                }
                Err(reason) => {
                    context.log(LogLevel::Warn, format!("the copy was refused: {reason}"));
                }
            }
            self.show(context);
            return true;
        }
        let Some(export) = self.export.as_mut() else {
            return false;
        };
        if action == action_id("export-confirm") || action == action_id("export-retry") {
            export.begin(context);
            self.show(context);
            return true;
        }
        false
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
    use super::{decode, encode, item_name, tag_action, Item, Todo, ADD, CLEAR_DONE, ITEMS};
    use super::{today, when, View, DUE_TODAY, EDIT_LIST};
    use kobo_sdk::{action_id, AppRunner, Command, Context, KoboApp, StoreRequest, StoreResult};
    use kobo_ui::{DiagnosticSeverity, LayoutKind, TextScale};

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
                    due: None,
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
                due: None,
            },
            Item {
                text: "a book with spaces and - dashes".into(),
                done: false,
                // A date on one item and none on the other, because the two
                // shapes of line have to survive the same round trip.
                due: Some(20_000),
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

    /// Every screen this application draws, for a list with something on it.
    fn every_screen(todo: &Todo) -> Vec<(String, kobo_sdk::Screen)> {
        let mut screens = vec![
            ("list".to_owned(), todo.list()),
            ("editing".to_owned(), todo.editing()),
        ];
        for index in 0..todo.items.len() {
            screens.push((format!("item-{index}"), todo.item(index)));
        }
        screens
    }

    #[test]
    fn every_screen_fits_the_panel_at_every_text_size() {
        // The count of rows a page holds used to be a constant. Six fits a
        // Clara BW at the default setting and four fit it at 170%, so a list
        // of long items drew its last rows through the buttons under them and
        // the renderer refused the whole screen: a reader who turned the type
        // up got a blank panel. Everything is measured now, and this is what
        // says so.
        let long = "book the van for the weekend and remember the deposit receipt \
                    from last time #errands";
        let mut failures = Vec::new();
        for scale in TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale: scale,
                ..kobo_ui::CLARA_BW_METRICS
            };
            let runner = AppRunner::with_metrics(Todo::default(), metrics);
            let mut items = vec![
                ("buy milk #shopping", false),
                (long, false),
                ("water the mint #garden", false),
                ("ring the vet", true),
            ];
            // Longer than any panel holds, so both lists really do page.
            for _ in 0..16 {
                items.push(("something else to do #later", false));
            }
            let mut todo = started(&items);
            todo.items[0].due = Some(today());
            todo.repaginate(&runner.context());
            for page in 0..todo.pages.len().max(todo.edit_pages.len()) {
                todo.page = page.min(todo.pages.len() - 1);
                todo.edit_page = page.min(todo.edit_pages.len() - 1);
                for (name, screen) in every_screen(&todo) {
                    let errors = screen
                        .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(false))
                        .issues
                        .into_iter()
                        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                        .map(|issue| format!("{:?}", issue.kind))
                        .collect::<Vec<_>>();
                    if !errors.is_empty() {
                        failures.push(format!("{scale:?} {name} page {page}: {errors:?}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_date_is_said_against_today_rather_than_as_a_date() {
        // A list of things to do is read against today and nothing else.
        assert_eq!(when(20_000, 20_000), "due today");
        assert_eq!(when(20_001, 20_000), "due tomorrow");
        assert_eq!(when(19_999, 20_000), "a day late");
        assert_eq!(when(19_997, 20_000), "3 days late");
        assert_eq!(when(20_003, 20_000), "due in 3 days");
        assert_eq!(when(20_030, 20_000), "due in 4 weeks");
    }

    #[test]
    fn a_date_survives_being_written_down_and_a_list_without_dates_still_reads() {
        let dated = vec![Item {
            text: "water the mint".into(),
            done: false,
            due: Some(20_000),
        }];
        assert_eq!(decode(&encode(&dated)), dated);
        // The format the version before dates wrote. Nobody's list has to be
        // migrated, so this still reads, and reads as having no date.
        let old = decode(b"- water the mint\n");
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].due, None);
    }

    #[test]
    fn clearing_the_finished_items_can_be_undone_until_the_next_change() {
        let mut todo = started(&[("milk", true), ("bread", false)]);
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(CLEAR_DONE));
        assert_eq!(todo.items.len(), 1);
        todo.on_action(&mut context, action_id(super::UNDO_CLEAR));
        assert_eq!(todo.items.len(), 2, "the cleared item did not come back");
        assert!(todo.items.iter().any(|item| item.text == "milk"));

        // And the offer goes as soon as anything else happens, because an
        // undo that outlives what it was undoing is a trap.
        todo.on_action(&mut context, action_id(CLEAR_DONE));
        todo.on_action(&mut context, action_id(&item_name(0)));
        assert!(todo.cleared.is_empty());
    }

    #[test]
    fn an_item_is_renamed_moved_and_dated_without_leaving_its_own_screen() {
        let mut todo = started(&[("first", false), ("second", false)]);
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(EDIT_LIST));
        assert_eq!(todo.view, View::Editing);
        todo.on_action(&mut context, action_id(&super::edit_name(1)));
        assert_eq!(todo.view, View::Item(1));

        todo.on_action(&mut context, action_id(DUE_TODAY));
        assert_eq!(todo.items[1].due, Some(today()));

        todo.on_action(&mut context, action_id(super::MOVE_UP));
        assert_eq!(todo.items[0].text, "second", "the item did not move");
        assert_eq!(
            todo.view,
            View::Item(0),
            "the screen followed something else after the move"
        );

        todo.on_action(&mut context, action_id(super::EDIT));
        assert!(todo.entry.is_open(), "renaming did not raise the keyboard");
        assert_eq!(
            todo.entry.text(),
            "second",
            "the field did not open on the words already written"
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
        // More than any panel holds, so the reader really is on a page that
        // the clearing takes away.
        let mut items: Vec<(&str, bool)> = Vec::new();
        for _ in 0..24 {
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

    #[test]
    fn a_tag_chip_narrows_the_list_to_the_tag_the_owner_wrote() {
        let mut todo = started(&[("milk #shop", false), ("bank #errand", false)]);
        assert_eq!(todo.all_tags(), vec!["shop", "errand"]);
        let mut context = Context::default();
        todo.on_action(&mut context, action_id(&tag_action("shop")));
        assert_eq!(todo.filter.as_deref(), Some("shop"));
        assert_eq!(todo.ordered(), vec![0], "the filter kept the wrong items");
        // Tapping the same chip again lets the whole list back.
        todo.on_action(&mut context, action_id(&tag_action("shop")));
        assert!(todo.filter.is_none());
    }
}
