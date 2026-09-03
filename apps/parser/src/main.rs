//! Parser is an offline, touch-first Z-machine interactive-fiction player.

mod zvm;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, ParagraphPresentation, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, TileShape,
};
use std::fmt::Write as _;
use std::process::ExitCode;
use zvm::{Machine, RunState, StoryInfo};

const STORY_PREFIX: &str = "story-";
const SAVE_PREFIX: &str = "save-";
const TRANSCRIPT_PAGE_BYTES: usize = 2_400;

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
    machine: Option<Machine>,
    open_blob: Option<String>,
    loading: Option<ShelfDownload>,
    saving: Option<ShelfUpload>,
    pending_restore: Option<ShelfDownload>,
    transcript: String,
    page: usize,
    keyboard: Keyboard,
    message: Option<String>,
    slot_action: SlotAction,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            view: View::Library,
            stories: Vec::new(),
            machine: None,
            open_blob: None,
            loading: None,
            saving: None,
            pending_restore: None,
            transcript: String::new(),
            page: 0,
            keyboard: Keyboard::new(),
            message: None,
            slot_action: SlotAction::Save,
        }
    }
}

impl Parser {
    fn show(&self, context: &mut Context) {
        context.set_screen(match self.view {
            View::Library => self.library_screen(),
            View::Play => self.play_screen(),
            View::Slots => self.slots_screen(),
        });
    }

    fn library_screen(&self) -> Screen {
        let mut builder = ScreenBuilder::new("parser")
            .top_bar("Parser")
            .heading("Interactive fiction")
            .text(
                "Push a .z3, .z5 or .z8 story with `kobo parser push FILE --device IP`. \
                 Stories play completely offline.",
            );
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
                .empty_state("No stories have been transferred to this reader.")
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
        let pages = transcript_pages(&self.transcript);
        let page = self.page.min(pages.len().saturating_sub(1));
        let text = pages.get(page).copied().unwrap_or("");
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
            .typed(&self.keyboard, "Type a command")
            .keyboard(&self.keyboard, "Run")
            .grid(
                4,
                false,
                [
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
                ],
            )
            .page_turns("page-back", "page-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        if let Some(message) = &self.message {
            builder = builder.banner(kobo_sdk::BannerLevel::Attention, message);
        }
        builder.build()
    }

    fn slots_screen(&self) -> Screen {
        let (title, instruction) = match self.slot_action {
            SlotAction::Save => ("Save game", "Tap to save the current turn"),
            SlotAction::Restore => ("Restore game", "Tap to restore this slot"),
        };
        let rows = (1..=10).map(|slot| {
            (
                format!("slot-{slot}"),
                format!("Slot {slot}"),
                instruction,
                Glyph::Bookmark,
            )
        });
        ScreenBuilder::new("parser-slots")
            .top_bar(title)
            .top_bar_action("play", "Back")
            .text("Parser keeps ten Quetzal slots per story. Autosave is separate and updated after every turn.")
            .rows(rows)
            .build()
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
                self.page = transcript_pages(&self.transcript).len().saturating_sub(1);
            }
            Err(error) => {
                self.transcript.push_str(&machine.take_output());
                self.message = Some(error.to_string());
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
            Ok(_) => {
                self.transcript.push_str(&machine.take_output());
                self.page = transcript_pages(&self.transcript).len().saturating_sub(1);
                self.autosave(context);
            }
            Err(error) => self.message = Some(error.to_string()),
        }
        self.show(context);
    }

    fn autosave(&mut self, context: &mut Context) {
        let Some(machine) = &self.machine else {
            return;
        };
        self.begin_save(context, save_name(machine.info(), "auto"));
    }

    fn begin_save(&mut self, context: &mut Context, name: String) {
        let Some(machine) = &self.machine else {
            return;
        };
        let mut upload = ShelfUpload::new(name, machine.save_quetzal());
        upload.start(context);
        self.saving = Some(upload);
    }

    fn noun(&mut self, index: usize) {
        let pages = transcript_pages(&self.transcript);
        let page = self.page.min(pages.len().saturating_sub(1));
        let links = pages
            .get(page)
            .map_or_else(Vec::new, |text| word_links(text));
        let Some((_, _, _, word)) = links.get(index).cloned() else {
            return;
        };
        let mut input = self.keyboard.text().to_owned();
        if !input.is_empty() && !input.ends_with(' ') {
            input.push(' ');
        }
        input.push_str(word.trim_matches(|character: char| !character.is_alphanumeric()));
        self.keyboard = Keyboard::with_text(input);
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
                                let mut restore = ShelfDownload::new(name).at_most(2 * 1024 * 1024);
                                restore.start(context);
                                self.pending_restore = Some(restore);
                                self.message = None;
                            }
                        }
                        self.view = View::Play;
                        self.show(context);
                    }
                    return;
                }
            }
        }
        if self.view != View::Play {
            return;
        }
        if let Some(pressed) = self.keyboard.press(action) {
            if pressed == Pressed::Submitted {
                let input = self.keyboard.take();
                self.command(context, &input);
            } else {
                self.show(context);
            }
            return;
        }
        let pages = transcript_pages(&self.transcript);
        let page = self.page.min(pages.len().saturating_sub(1));
        for (index, (name, _, _, _)) in pages
            .get(page)
            .map_or_else(Vec::new, |text| word_links(text))
            .iter()
            .enumerate()
        {
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
            self.page = (self.page + 1).min(pages.len().saturating_sub(1));
            self.show(context);
            return;
        }
        if action == action_id("save") {
            self.slot_action = SlotAction::Save;
            self.view = View::Slots;
            self.show(context);
            return;
        }
        if action == action_id("restore") {
            self.slot_action = SlotAction::Restore;
            self.view = View::Slots;
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
                    self.show(context);
                } else {
                    self.command(context, command);
                }
                return;
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view != View::Play {
            return;
        }
        let pages = transcript_pages(&self.transcript);
        self.page = if forward {
            (self.page + 1).min(pages.len().saturating_sub(1))
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
            if self.view == View::Library {
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
                    if let Some(machine) = &mut self.machine {
                        if let Err(error) = machine.restore_quetzal(&bytes) {
                            self.message = Some(error.to_string());
                        }
                    }
                    self.advance_story(context);
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

fn transcript_pages(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec!["Starting story…"];
    }
    let mut pages = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + TRANSCRIPT_PAGE_BYTES).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len() {
            if let Some(break_at) = text[start..end].rfind("\n\n") {
                end = start + break_at + 2;
            } else if let Some(break_at) = text[start..end].rfind('\n') {
                end = start + break_at + 1;
            }
        }
        if end == start {
            end = text.len();
        }
        pages.push(&text[start..end]);
        start = end;
    }
    pages
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
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn transcript_navigation_is_utf8_safe_and_preserves_all_text() {
        let text = format!("{}\n\n{}", "word ".repeat(600), "café ".repeat(600));
        let pages = transcript_pages(&text);
        assert!(pages.len() > 1);
        assert_eq!(pages.concat(), text);
    }

    #[test]
    fn transcript_words_are_tappable_and_appendable() {
        let links = word_links("Take the brass-lamp, please.");
        assert!(links.iter().any(|link| link.3 == "brass-lamp"));
        let mut parser = Parser {
            transcript: "A brass lamp waits.".to_owned(),
            ..Parser::default()
        };
        parser.noun(0);
        assert_eq!(parser.keyboard.text(), "brass");
    }

    #[test]
    fn library_and_play_layouts_fit_clara() {
        let mut parser = Parser::default();
        parser.stories.push(("story-advent.z3".to_owned(), 128_000));
        for screen in [parser.library_screen(), parser.play_screen()] {
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        }
    }
}
