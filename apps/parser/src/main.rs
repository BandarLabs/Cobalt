//! Parser is an offline, touch-first Z-machine interactive-fiction player.

mod zvm;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Chrome, Context, DisplayMetrics, Glyph, KoboApp, LayoutIssueKind,
    ParagraphPresentation, Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload,
    StoreResult, TileShape,
};
use std::fmt::Write as _;
use std::process::ExitCode;
use zvm::{Machine, RunState, StoryInfo};

// Three rows to a page rather than four: with the story's own checkpoint
// listed and a message on the panel, four rows crowded the guidance line off
// the largest text scale, and the renderer refused the screen.
const SLOT_PAGE_ROWS: usize = 3;

const STORY_PREFIX: &str = "story-";
const SAVE_PREFIX: &str = "save-";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Library,
    Play,
    Slots,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotAction {
    Save,
    Restore,
}

struct Parser {
    view: View,
    stories: Vec<(String, u32)>,
    saves: Vec<String>,
    slots_page: usize,
    machine: Option<Machine>,
    open_blob: Option<String>,
    loading: Option<ShelfDownload>,
    saving: Option<ShelfUpload>,
    pending_restore: Option<ShelfDownload>,
    transcript: String,
    pages: Vec<(usize, usize)>,
    paginated_len: usize,
    pages_metrics: Option<DisplayMetrics>,
    pages_keyboard_open: Option<bool>,
    page: usize,
    keyboard: Keyboard,
    message: Option<String>,
    slot_action: SlotAction,
    keyboard_open: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            view: View::Library,
            stories: Vec::new(),
            saves: Vec::new(),
            slots_page: 0,
            machine: None,
            open_blob: None,
            loading: None,
            saving: None,
            pending_restore: None,
            transcript: String::new(),
            pages: Vec::new(),
            paginated_len: 0,
            pages_metrics: None,
            pages_keyboard_open: None,
            page: 0,
            keyboard: Keyboard::new(),
            message: None,
            slot_action: SlotAction::Save,
            keyboard_open: false,
        }
    }
}

impl Parser {
    fn show(&mut self, context: &mut Context) {
        self.repaginate(context);
        context.set_screen(match self.view {
            View::Library => self.library_screen(),
            View::Play => self.play_screen(),
            View::Slots => self.slots_screen(),
        });
    }

    fn library_screen(&self) -> Screen {
        let mut builder = ScreenBuilder::new("parser")
            .top_bar("Parser")
            .heading("Interactive fiction");
        if let Some(message) = &self.message {
            builder = builder.banner(kobo_sdk::BannerLevel::Attention, message);
        }
        let stories = self
            .stories
            .iter()
            .enumerate()
            .map(|(index, (name, size))| {
                (
                    format!("story-{index}"),
                    display_name(name),
                    Glyph::Book,
                    move |tile: kobo_sdk::Tile| tile.with_subtitle(format_size(*size)),
                )
            })
            .collect::<Vec<_>>();
        if stories.is_empty() {
            builder
                .empty_state(
                    "No stories yet. Push a .z3, .z5 or .z8 story with \
                     `kobo parser push FILE --device IP`; stories play completely offline.",
                )
                .bottom_action("refresh", "Refresh library")
                .build()
        } else {
            builder
                .tile_grid(TileShape::Portrait, stories)
                .bottom_action("refresh", "Refresh library")
                .build()
        }
    }

    fn play_screen(&self) -> Screen {
        if self.transcript.is_empty() {
            return self.play_screen_for("Starting story…", 1, 1);
        }
        let index = self.page.min(self.pages.len().saturating_sub(1));
        let text = self
            .pages
            .get(index)
            .and_then(|&(start, end)| self.transcript.get(start..end))
            .unwrap_or("");
        self.play_screen_for(text, index + 1, self.pages.len().max(1))
    }

    fn play_screen_for(&self, text: &str, page: usize, pages: usize) -> Screen {
        let links = word_links(text);
        let status = match self.machine.as_ref() {
            Some(machine) => {
                if machine.status().is_empty() {
                    machine.info().title.clone()
                } else {
                    machine.status().to_owned()
                }
            }
            None => "Parser".to_owned(),
        };
        let commands = [
            ("look", "LOOK"),
            ("inventory", "INVENTORY"),
            ("examine", "EXAMINE"),
            ("take", "TAKE"),
            ("north", "N"),
            ("south", "S"),
            ("east", "E"),
            ("west", "W"),
            ("undo", "UNDO"),
            ("save", "SAVE"),
            ("restore", "RESTORE"),
            ("again", "AGAIN"),
        ];
        let mut builder = ScreenBuilder::new("parser-play")
            .top_bar(status)
            .top_bar_glyph("library", "Library", Glyph::Book)
            .reading(true)
            .rich_text_linking(
                text,
                Vec::new(),
                ParagraphPresentation::default(),
                links
                    .iter()
                    .map(|(name, start, end, _)| (name, *start, *end)),
            )
            .divider()
            .typed(&self.keyboard, "Type a command");
        // One input surface at a time: the palette by default, the keyboard
        // behind a top-bar toggle, so the screen fits every panel and pose.
        builder = builder.top_bar_action(
            "keyboard-toggle",
            if self.keyboard_open {
                "Close keys"
            } else {
                "Keyboard"
            },
        );
        builder = if self.keyboard_open {
            builder.keyboard(&self.keyboard, "Run")
        } else {
            builder.grid(4, false, commands)
        };
        builder = builder.page_turns("page-back", "page-next").page_position(
            u16::try_from(page).unwrap_or(u16::MAX),
            u16::try_from(pages).unwrap_or(u16::MAX),
        );
        if let Some(message) = &self.message {
            builder = builder.banner(kobo_sdk::BannerLevel::Attention, message);
        }
        builder.build()
    }

    fn slots_screen(&self) -> Screen {
        let title = match self.slot_action {
            SlotAction::Save => "Save game",
            SlotAction::Restore => "Restore game",
        };
        let page_count = 10usize.div_ceil(SLOT_PAGE_ROWS);
        let page = self.slots_page.min(page_count - 1);
        let first = page * SLOT_PAGE_ROWS + 1;
        let last = (first + SLOT_PAGE_ROWS - 1).min(10);
        let rows = (first..=last).map(|slot| {
            let occupied = self.machine.as_ref().is_some_and(|machine| {
                self.saves
                    .contains(&save_name(machine.info(), &slot.to_string()))
            });
            (
                format!("slot-{slot}"),
                format!("Slot {slot}"),
                slot_subtitle(self.slot_action, occupied),
                Glyph::Bookmark,
            )
        });
        // The story's own checkpoint (its "save" command) is restorable from
        // here too, so a story-initiated restore can find it.
        let checkpoint = if self.slot_action == SlotAction::Restore {
            self.machine
                .as_ref()
                .map(|machine| save_name(machine.info(), "game"))
                .filter(|name| self.saves.contains(name))
        } else {
            None
        };
        let rows: Vec<_> = checkpoint
            .into_iter()
            .map(|_| {
                (
                    "story-checkpoint".to_owned(),
                    "Story checkpoint".to_owned(),
                    "Saved by the story itself".to_owned(),
                    Glyph::Bookmark,
                )
            })
            .chain(rows)
            .collect();
        // One line up top says what a slot is before anybody has to guess:
        // a position of this story kept on this reader, with the story
        // itself resuming where it was left whether a slot was used or not.
        let guidance = match self.slot_action {
            SlotAction::Save => {
                "Keep this position in a slot. The story also resumes where you left off."
            }
            SlotAction::Restore => {
                "Return to a position you kept. The newest play waits where it is."
            }
        };
        let mut builder = ScreenBuilder::new("parser-slots")
            .top_bar(title)
            .top_bar_action("play", "Back")
            .text(guidance)
            .rows(rows);
        if page_count > 1 {
            builder = builder
                .page_turns("slots-page-back", "slots-page-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(page_count).unwrap_or(u16::MAX),
                );
        }
        if let Some(message) = &self.message {
            builder = builder.banner(kobo_sdk::BannerLevel::Attention, message);
        }
        builder.build()
    }

    fn open_story(&mut self, context: &mut Context, index: usize) {
        let Some((name, _)) = self.stories.get(index) else {
            return;
        };
        self.message = None;
        let mut download = ShelfDownload::new(name).at_most(16 * 1024 * 1024);
        download.start(context);
        self.loading = Some(download);
    }

    fn start_story(&mut self, context: &mut Context, blob: String, bytes: Vec<u8>) {
        match Machine::new(bytes, &blob) {
            Ok(machine) => {
                let save = save_name(machine.info(), "auto");
                self.machine = Some(machine);
                self.open_blob = Some(blob);
                self.transcript.clear();
                self.pages.clear();
                self.paginated_len = 0;
                self.page = 0;
                self.view = View::Play;
                let mut restore = ShelfDownload::new(save).at_most(2 * 1024 * 1024);
                restore.start(context);
                self.pending_restore = Some(restore);
                self.show(context);
            }
            Err(error) => {
                self.message = Some(error.to_string());
                self.view = View::Library;
                self.show(context);
            }
        }
    }

    fn advance_story(&mut self, context: &mut Context) {
        let Some(machine) = &mut self.machine else {
            return;
        };
        match machine.run() {
            Ok(state) => {
                self.transcript.push_str(&machine.take_output());
                if state == RunState::Halted {
                    self.transcript.push_str("\n\n[The story has ended.]\n");
                }
                self.repaginate(context);
                self.page = self.last_content_page();
                self.finish_file_request(context, &state);
            }
            Err(error) => {
                self.transcript.push_str(&machine.take_output());
                self.message = Some(error.to_string());
                self.repaginate(context);
            }
        }
        self.show(context);
    }

    fn command(&mut self, context: &mut Context, command: &str) {
        if command.trim().is_empty() {
            return;
        }
        let Some(machine) = &mut self.machine else {
            return;
        };
        let _ = write!(self.transcript, "\n> {}\n", command.trim());
        match machine.input(command.trim()) {
            Ok(state) => {
                self.transcript.push_str(&machine.take_output());
                self.repaginate(context);
                self.page = self.last_content_page();
                self.finish_file_request(context, &state);
                self.autosave(context);
            }
            Err(error) => {
                self.message = Some(error.to_string());
                self.repaginate(context);
            }
        }
        self.show(context);
    }

    fn autosave(&mut self, context: &mut Context) {
        let Some(machine) = &self.machine else {
            return;
        };
        self.begin_save(context, save_name(machine.info(), "auto"));
    }

    /// The story asked to save or restore a game itself (0OP 5/6). Save:
    /// persist the suspended state as the "game" checkpoint, then let the
    /// story continue. Restore: no file is picked here - the reader uses
    /// the slot picker - so the story resumes with the spec's failure
    /// result instead of hanging.
    fn finish_file_request(&mut self, context: &mut Context, state: &RunState) {
        if *state == RunState::NeedSave {
            let name = self
                .machine
                .as_ref()
                .map(|machine| save_name(machine.info(), "game"));
            if let Some(name) = name {
                // Optimistic, like the menu's own "Saved in slot" message:
                // the restore picker must see the checkpoint immediately.
                if !self.saves.contains(&name) {
                    self.saves.push(name.clone());
                }
                self.begin_save(context, name);
            }
            if let Some(machine) = &mut self.machine {
                match machine.complete_save(true) {
                    Ok(state) => {
                        self.transcript.push_str(&machine.take_output());
                        self.message = Some("Saved the story checkpoint.".to_owned());
                        self.repaginate(context);
                        self.page = self.last_content_page();
                        if !matches!(state, RunState::Halted | RunState::NeedInput { .. }) {
                            self.finish_file_request(context, &state);
                        }
                    }
                    Err(error) => self.message = Some(error.to_string()),
                }
            }
        } else if *state == RunState::NeedRestore {
            // Route the story's restore to the slot picker. Loading a slot
            // resolves the suspension on restore_quetzal; an empty slot or
            // backing out resumes the story with the spec's failure result.
            self.slot_action = SlotAction::Restore;
            self.view = View::Slots;
            self.slots_page = 0;
            self.message = Some("The story asked to restore a game - pick a slot.".to_owned());
        }
    }

    fn begin_save(&mut self, context: &mut Context, name: String) {
        let Some(machine) = &self.machine else {
            return;
        };
        let mut upload = ShelfUpload::new(name, machine.save_quetzal());
        upload.start(context);
        self.saving = Some(upload);
    }

    fn last_content_page(&self) -> usize {
        let mut page = self.pages.len().saturating_sub(1);
        while page > 0 {
            let (start, end) = self.pages[page];
            if self.transcript.get(start..end).map_or("", str::trim) != ">" {
                break;
            }
            page -= 1;
        }
        page
    }

    fn note_restored(&mut self) {
        if !self.transcript.is_empty() {
            self.transcript.push_str("\n\n");
        }
        self.transcript.push_str("[Restored.]");
    }

    fn noun(&mut self, index: usize) {
        let page = self.page.min(self.pages.len().saturating_sub(1));
        let text = self
            .pages
            .get(page)
            .and_then(|&(start, end)| self.transcript.get(start..end))
            .unwrap_or("");
        let links = word_links(text);
        let Some((_, _, _, word)) = links.get(index).cloned() else {
            return;
        };
        self.keyboard_open = true;
        let mut input = self.keyboard.text().to_owned();
        if !input.is_empty() && !input.ends_with(' ') {
            input.push(' ');
        }
        input.push_str(word.trim_matches(|character: char| !character.is_alphanumeric()));
        self.keyboard = Keyboard::with_text(input);
    }
}

impl Parser {
    fn play_page_fits(&self, text: &str, metrics: DisplayMetrics) -> bool {
        let diagnostics = self
            .play_screen_for(text, 1, 2)
            .diagnostics(&metrics, &Chrome::measuring(true));
        !diagnostics.issues.iter().any(|issue| {
            matches!(
                issue.kind,
                LayoutIssueKind::ContentOverflow { .. }
                    | LayoutIssueKind::Clipped
                    | LayoutIssueKind::TextOverflow
                    | LayoutIssueKind::TouchTargetTooSmall { .. }
            )
        })
    }

    fn repaginate(&mut self, context: &Context) {
        self.repaginate_for_metrics(context.metrics());
    }

    fn repaginate_for_metrics(&mut self, metrics: DisplayMetrics) {
        if self.pages_metrics != Some(metrics)
            || self.pages_keyboard_open != Some(self.keyboard_open)
            || self.paginated_len > self.transcript.len()
            || !self.transcript.is_char_boundary(self.paginated_len)
        {
            self.pages.clear();
            self.paginated_len = 0;
        }
        self.pages_metrics = Some(metrics);
        self.pages_keyboard_open = Some(self.keyboard_open);
        if self.paginated_len == self.transcript.len() {
            self.page = self.page.min(self.pages.len().saturating_sub(1));
            return;
        }
        let mut start = self.pages.pop().map_or(0, |(page_start, _)| page_start);
        let text = &self.transcript;
        let boundaries: Vec<usize> = text
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(text.len()))
            .collect();
        while start < text.len() {
            let start_index = boundaries
                .binary_search(&start)
                .expect("page start is a char boundary");
            let mut low = start_index + 1;
            let mut high = boundaries.len() - 1;
            let mut best = boundaries[low.min(high)];
            while low <= high {
                let middle = low + (high - low) / 2;
                let end = boundaries[middle];
                if self.play_page_fits(&text[start..end], metrics) {
                    best = end;
                    low = middle + 1;
                } else if middle == 0 {
                    break;
                } else {
                    high = middle - 1;
                }
            }
            if best < text.len() {
                let segment = &text[start..best];
                let half = segment.len() / 2;
                let break_at = segment
                    .rfind("\n\n")
                    .filter(|offset| *offset >= half)
                    .map(|offset| offset + 2)
                    .or_else(|| {
                        segment
                            .rfind('\n')
                            .filter(|offset| *offset >= half)
                            .map(|offset| offset + 1)
                    });
                if let Some(offset) = break_at {
                    best = start + offset;
                }
            }
            if best <= start {
                best = text.len();
            }
            self.pages.push((start, best));
            start = best;
        }
        self.paginated_len = self.transcript.len();
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }
}

impl KoboApp for Parser {
    fn on_start(&mut self, context: &mut Context) {
        context.shelf().list();
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("library") {
            self.view = View::Library;
            context.shelf().list();
            self.show(context);
            return;
        }
        if action == action_id("play") {
            if self.view == View::Slots {
                if let Some(machine) = &mut self.machine {
                    if machine.awaiting_restore() {
                        if let Ok(state) = machine.complete_restore(false) {
                            self.transcript.push_str(&machine.take_output());
                            self.repaginate(context);
                            self.page = self.last_content_page();
                            self.finish_file_request(context, &state);
                        }
                    }
                }
            }
            self.view = View::Play;
            self.show(context);
            return;
        }
        if action == action_id("refresh") {
            context.shelf().list();
            return;
        }
        for index in 0..self.stories.len() {
            if action == action_id(&format!("story-{index}")) {
                self.open_story(context, index);
                return;
            }
        }
        if self.view == View::Slots {
            if action == action_id("story-checkpoint") {
                if let Some(machine) = &self.machine {
                    let name = save_name(machine.info(), "game");
                    let mut restore = ShelfDownload::new(name).at_most(2 * 1024 * 1024);
                    restore.start(context);
                    self.pending_restore = Some(restore);
                    self.message = None;
                    self.view = View::Play;
                    self.show(context);
                }
                return;
            }
            for slot in 1..=10 {
                if action == action_id(&format!("slot-{slot}")) {
                    if let Some(machine) = &self.machine {
                        let name = save_name(machine.info(), &slot.to_string());
                        match self.slot_action {
                            SlotAction::Save => {
                                self.begin_save(context, name);
                                self.message = Some(format!("Saved in slot {slot}."));
                            }
                            SlotAction::Restore => {
                                if self.saves.contains(&name) {
                                    let mut restore =
                                        ShelfDownload::new(name).at_most(2 * 1024 * 1024);
                                    restore.start(context);
                                    self.pending_restore = Some(restore);
                                    self.message = None;
                                    self.view = View::Play;
                                } else {
                                    self.message = Some(format!("Slot {slot} is empty."));
                                }
                            }
                        }
                        if self.slot_action == SlotAction::Restore
                            && self.pending_restore.is_none()
                            && self.machine.as_ref().is_some_and(Machine::awaiting_restore)
                        {
                            // Tapped an empty slot while the story waits on a
                            // restore: resume it as failed and go back.
                            if let Some(machine) = &mut self.machine {
                                if let Ok(state) = machine.complete_restore(false) {
                                    self.transcript.push_str(&machine.take_output());
                                    self.repaginate(context);
                                    self.page = self.last_content_page();
                                    self.view = View::Play;
                                    self.finish_file_request(context, &state);
                                }
                            }
                        }
                        if self.pending_restore.is_some() {
                            self.view = View::Play;
                        }
                        self.show(context);
                    }
                    return;
                }
            }
        }
        if self.view != View::Play {
            return;
        }
        if action == action_id("keyboard-toggle") {
            self.keyboard_open = !self.keyboard_open;
            self.show(context);
            return;
        }
        if self.keyboard_open {
            if let Some(pressed) = self.keyboard.press(action) {
                if pressed == Pressed::Submitted {
                    let input = self.keyboard.take();
                    self.command(context, &input);
                } else {
                    self.show(context);
                }
                return;
            }
        }
        let page = self.page.min(self.pages.len().saturating_sub(1));
        let text = self
            .pages
            .get(page)
            .and_then(|&(start, end)| self.transcript.get(start..end))
            .unwrap_or("");
        for (index, (name, _, _, _)) in word_links(text).iter().enumerate() {
            if action == action_id(name) {
                self.noun(index);
                self.show(context);
                return;
            }
        }
        if action == action_id("page-back") {
            self.page = self.page.saturating_sub(1);
            self.show(context);
            return;
        }
        if action == action_id("page-next") {
            self.page = (self.page + 1).min(self.pages.len().saturating_sub(1));
            self.show(context);
            return;
        }
        if action == action_id("save") {
            self.slot_action = SlotAction::Save;
            self.slots_page = 0;
            self.view = View::Slots;
            context.shelf().list();
            self.show(context);
            return;
        }
        if action == action_id("restore") {
            self.slot_action = SlotAction::Restore;
            self.slots_page = 0;
            self.view = View::Slots;
            context.shelf().list();
            self.show(context);
            return;
        }
        for (name, command) in [
            ("look", "look"),
            ("inventory", "inventory"),
            ("examine", "examine "),
            ("take", "take "),
            ("north", "north"),
            ("south", "south"),
            ("east", "east"),
            ("west", "west"),
            ("undo", "undo"),
            ("again", "again"),
        ] {
            if action == action_id(name) {
                if command.ends_with(' ') {
                    self.keyboard = Keyboard::with_text(command);
                    self.keyboard_open = true;
                    self.show(context);
                } else {
                    self.command(context, command);
                }
                return;
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Slots {
            let page_count = 10usize.div_ceil(SLOT_PAGE_ROWS);
            self.slots_page = if forward {
                (self.slots_page + 1).min(page_count - 1)
            } else {
                self.slots_page.saturating_sub(1)
            };
            self.show(context);
            return;
        }
        if self.view != View::Play {
            return;
        }
        self.page = if forward {
            (self.page + 1).min(self.pages.len().saturating_sub(1))
        } else {
            self.page.saturating_sub(1)
        };
        self.show(context);
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.autosave(context);
    }

    fn on_background(&mut self, context: &mut Context) {
        self.autosave(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.autosave(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Shelf(items) = &result {
            self.stories = items
                .iter()
                .filter(|(name, _)| name.starts_with(STORY_PREFIX))
                .cloned()
                .collect();
            self.stories.sort_by(|left, right| left.0.cmp(&right.0));
            self.saves = items
                .iter()
                .filter(|(name, _)| name.starts_with(SAVE_PREFIX))
                .map(|(name, _)| name.clone())
                .collect();
            if self.view == View::Library || self.view == View::Slots {
                self.show(context);
            }
            return;
        }
        if let Some(upload) = &mut self.saving {
            match upload.advance(context, &result) {
                ShelfProgress::Done => {
                    self.saving = None;
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.saving = None;
                    self.message =
                        Some("The save could not be written. Check free space.".to_owned());
                    self.show(context);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.loading {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    let download = self.loading.take().expect("story download exists");
                    let name = download.name().to_owned();
                    self.start_story(context, name, download.take());
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.loading = None;
                    self.message = Some("That story could not be read from storage.".to_owned());
                    self.show(context);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.pending_restore {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    let bytes = self
                        .pending_restore
                        .take()
                        .expect("restore download exists")
                        .take();
                    let mut restored = false;
                    if let Some(machine) = &mut self.machine {
                        match machine.restore_quetzal(&bytes) {
                            Ok(()) => restored = true,
                            Err(error) => self.message = Some(error.to_string()),
                        }
                    }
                    if restored {
                        self.note_restored();
                    }
                    self.advance_story(context);
                    if restored {
                        self.command(context, "look");
                    }
                }
                ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                    self.pending_restore = None;
                    self.advance_story(context);
                }
                ShelfProgress::Failed(_) => {
                    self.pending_restore = None;
                    self.message = Some("The saved game could not be restored.".to_owned());
                    self.advance_story(context);
                }
                ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
            }
        }
    }
}

fn word_links(text: &str) -> Vec<(String, usize, usize, String)> {
    let mut links = Vec::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if character.is_alphanumeric() || character == '\'' || character == '-' {
            start.get_or_insert(index);
        } else if let Some(from) = start.take() {
            if index > from + 1 && links.len() < 16 {
                links.push((
                    format!("noun-{}", links.len()),
                    from,
                    index,
                    text[from..index].to_owned(),
                ));
            }
        }
    }
    links
}

fn slot_subtitle(action: SlotAction, occupied: bool) -> String {
    match (action, occupied) {
        (SlotAction::Save, true) => "Saved game: tap to overwrite".to_owned(),
        (SlotAction::Save, false) => "Empty: tap to save here".to_owned(),
        (SlotAction::Restore, true) => "Saved game: tap to restore".to_owned(),
        (SlotAction::Restore, false) => "Empty: nothing to restore".to_owned(),
    }
}

fn save_name(info: &StoryInfo, slot: &str) -> String {
    let mut id = info
        .id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    id.truncate(40);
    format!("{SAVE_PREFIX}{id}-{slot}")
}

fn display_name(name: &str) -> String {
    name.trim_start_matches(STORY_PREFIX)
        .replace(['_', '-'], " ")
}

fn format_size(bytes: u32) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", f64::from(bytes) / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("parser", Parser::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("parser: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

    /// Measure with the same face the runtime draws with, or the tests approve
    /// pages the panel cannot show.
    fn install_real_face() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            kobo_text::install(CLARA_BW_METRICS).expect("bundled face installs");
        });
    }

    #[test]
    fn transcript_pagination_is_measured_utf8_safe_and_preserves_all_text() {
        let text = format!("{}\n\n{}", "word ".repeat(600), "café ".repeat(600));
        let mut parser = Parser {
            transcript: text.clone(),
            ..Parser::default()
        };
        parser.repaginate_for_metrics(CLARA_BW_METRICS);
        assert!(parser.pages.len() > 1);
        let joined: String = parser
            .pages
            .iter()
            .map(|&(start, end)| &text[start..end])
            .collect();
        assert_eq!(joined, text);
        for &(start, end) in &parser.pages {
            assert!(text.is_char_boundary(start));
            assert!(text.is_char_boundary(end));
            assert!(parser.play_page_fits(&text[start..end], CLARA_BW_METRICS));
        }
    }

    #[test]
    fn new_output_lands_on_output_not_a_stranded_prompt() {
        let probe = Parser::default();
        let mut words = 1usize;
        while probe.play_page_fits(&"word ".repeat(words + 1), CLARA_BW_METRICS) {
            words += 1;
        }
        // A full page of output followed by the Z-machine prompt: the prompt
        // cannot fit, so pagination must strand it and landing must skip it.
        let text = format!("{}\n\n>", "word ".repeat(words));
        let mut parser = Parser {
            transcript: text.clone(),
            ..Parser::default()
        };
        parser.repaginate_for_metrics(CLARA_BW_METRICS);
        assert!(parser.pages.len() > 1);
        let (start, end) = parser.pages[parser.pages.len() - 1];
        assert_eq!(text[start..end].trim(), ">");
        let landing = parser.last_content_page();
        assert_eq!(landing, parser.pages.len() - 2);
        let (start, end) = parser.pages[landing];
        assert_ne!(text[start..end].trim(), ">");
    }

    fn zork1_fixture() -> Vec<u8> {
        include_bytes!("../fixtures/zork1.z3").to_vec()
    }

    #[test]
    fn zork1_fixture_facts_match_provenance() {
        let info = StoryInfo::inspect(&zork1_fixture(), "zork1.z3").expect("fixture inspects");
        assert_eq!(info.release, 119);
        assert_eq!(&info.serial, b"880429");
        assert_eq!(info.checksum, 0xbf44);
        assert_eq!(info.bytes, 86_838);
    }

    #[test]
    fn zork1_opens_answers_and_survives_a_save_round_trip() {
        let mut machine = Machine::new(zork1_fixture(), "zork1.z3").expect("fixture opens");
        machine.run().expect("opening runs");
        let opening = machine.take_output();
        assert!(opening.contains("West of House"), "{opening}");
        machine.input("look").expect("look accepted");
        let look = machine.take_output();
        assert!(look.contains("West of House"), "{look}");
        machine.input("inventory").expect("inventory accepted");
        let inventory = machine.take_output();
        assert!(inventory.contains("empty-handed"), "{inventory}");
        machine.input("open mailbox").expect("open accepted");
        machine.take_output();
        machine.input("take leaflet").expect("take accepted");
        machine.take_output();
        machine.input("inventory").expect("inventory accepted");
        let inventory = machine.take_output();
        assert!(inventory.contains("leaflet"), "{inventory}");
        let save = machine.save_quetzal();
        let mut restored = Machine::new(zork1_fixture(), "zork1.z3").expect("fixture reopens");
        restored.restore_quetzal(&save).expect("save restores");
        restored.input("look").expect("restored look accepted");
        let look = restored.take_output();
        assert!(look.contains("West of House"), "{look}");
    }

    #[test]
    fn zork1_opening_paginates_cleanly_and_lands_on_the_story() {
        install_real_face();
        let mut machine = Machine::new(zork1_fixture(), "zork1.z3").expect("fixture opens");
        machine.run().expect("opening runs");
        machine.input("look").expect("look accepted");
        let mut parser = Parser {
            transcript: machine.take_output(),
            ..Parser::default()
        };
        for keyboard_open in [false, true] {
            parser.keyboard_open = keyboard_open;
            for scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    text_scale: scale,
                    ..CLARA_BW_METRICS
                };
                parser.repaginate_for_metrics(metrics);
                for &(start, end) in &parser.pages {
                    let page_text = &parser.transcript[start..end];
                    assert!(
                        parser.play_page_fits(page_text, metrics),
                        "{scale:?} keyboard_open={keyboard_open} overflow: {page_text:?}"
                    );
                }
            }
            if keyboard_open {
                assert!(
                    parser.pages.len() > 1,
                    "real output spans pages with the keyboard open"
                );
            }
        }
        let landing = parser.last_content_page();
        let (start, end) = parser.pages[landing];
        assert_ne!(parser.transcript[start..end].trim(), ">");
    }

    #[test]
    fn zork1_landing_fits_elipsa_extra_large_with_display_chrome() {
        install_real_face();
        let mut machine = Machine::new(zork1_fixture(), "zork1.z3").expect("fixture opens");
        machine.run().expect("opening runs");
        let mut parser = Parser {
            transcript: machine.take_output(),
            ..Parser::default()
        };
        for keyboard_open in [false, true] {
            parser.keyboard_open = keyboard_open;
            let metrics = DisplayMetrics {
                width: 1404,
                height: 1872,
                pixels_per_inch: 227,
                text_scale: TextScale::ExtraLarge,
            };
            parser.repaginate_for_metrics(metrics);
            let landing = parser.last_content_page();
            let (start, end) = parser.pages[landing];
            let text = parser.transcript[start..end].to_owned();
            let screen = parser.play_screen_for(&text, landing + 1, parser.pages.len());
            for chrome in [Chrome::default(), Chrome::measuring(true)] {
                let diagnostics = screen.diagnostics(&metrics, &chrome);
                assert!(
                    diagnostics.issues.is_empty(),
                    "keyboard_open={keyboard_open}: {:?}",
                    diagnostics.issues
                );
            }
        }
    }

    fn shown(screen: &Screen) -> Vec<String> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    #[test]
    fn the_slots_screen_says_what_a_slot_is() {
        for (action, expected) in [
            (SlotAction::Save, "Keep this position in a slot"),
            (SlotAction::Restore, "Return to a position you kept"),
        ] {
            let parser = Parser {
                slot_action: action,
                ..Parser::default()
            };
            let lines = shown(&parser.slots_screen());
            assert!(
                lines.iter().any(|line| line.contains(expected)),
                "the {action:?} screen does not explain slots: {lines:?}"
            );
        }
    }

    #[test]
    fn slot_rows_name_what_each_slot_holds() {
        assert_eq!(
            slot_subtitle(SlotAction::Save, true),
            "Saved game: tap to overwrite"
        );
        assert_eq!(
            slot_subtitle(SlotAction::Save, false),
            "Empty: tap to save here"
        );
        assert_eq!(
            slot_subtitle(SlotAction::Restore, false),
            "Empty: nothing to restore"
        );
        let machine = Machine::new(zork1_fixture(), "zork1.z3").expect("fixture opens");
        let mut parser = Parser {
            saves: vec!["save-zork1-3".to_owned(), save_name(machine.info(), "game")],
            machine: Some(machine),
            ..Parser::default()
        };
        parser.slot_action = SlotAction::Restore;
        parser.message = Some("The story asked to restore a game - pick a slot.".to_owned());
        let page_count = 10usize.div_ceil(SLOT_PAGE_ROWS);
        for scale in TextScale::STEPS {
            let scaled = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for page in 0..page_count {
                parser.slots_page = page;
                let diagnostics = parser
                    .slots_screen()
                    .diagnostics(&scaled, &Chrome::default());
                assert!(
                    diagnostics.issues.is_empty(),
                    "page {page} at {scaled:?}: {:?}",
                    diagnostics.issues
                );
            }
        }
    }

    #[test]
    fn restore_marker_separates_old_transcript_from_new() {
        let mut parser = Parser::default();
        parser.note_restored();
        assert_eq!(parser.transcript, "[Restored.]");
        parser.note_restored();
        assert_eq!(parser.transcript, "[Restored.]\n\n[Restored.]");
    }

    #[test]
    fn transcript_words_are_tappable_and_appendable() {
        let links = word_links("Take the brass-lamp, please.");
        assert!(links.iter().any(|link| link.3 == "brass-lamp"));
        let mut parser = Parser {
            transcript: "A brass lamp waits.".to_owned(),
            ..Parser::default()
        };
        parser.pages = vec![(0, parser.transcript.len())];
        parser.paginated_len = parser.transcript.len();
        parser.noun(0);
        assert_eq!(parser.keyboard.text(), "brass");
    }

    #[test]
    fn supported_matrix_layouts_fit() {
        install_real_face();
        // One panel per supported geometry: Clara (both densities), Libra,
        // Elipsa, each in both poses, at every text scale.
        let mut parser = Parser::default();
        parser.stories.push(("story-advent.z3".to_owned(), 128_000));
        for (width, height, ppi) in [
            (1072, 1448, 300),
            (1072, 1448, 212),
            (1264, 1680, 300),
            (1404, 1872, 227),
        ] {
            for pose in [(width, height), (height, width)] {
                for scale in TextScale::STEPS {
                    let metrics = DisplayMetrics {
                        width: pose.0,
                        height: pose.1,
                        pixels_per_inch: ppi,
                        text_scale: scale,
                    };
                    for keyboard_open in [false, true] {
                        parser.keyboard_open = keyboard_open;
                        for (name, screen) in [
                            ("library", parser.library_screen()),
                            ("play", parser.play_screen()),
                        ] {
                            let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
                            assert!(
                                diagnostics.issues.is_empty(),
                                "{name} {}x{}@{ppi} {scale:?} keyboard_open={keyboard_open}: {:?}",
                                pose.0,
                                pose.1,
                                diagnostics.issues
                            );
                        }
                    }
                }
            }
        }
    }
}
