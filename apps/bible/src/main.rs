#![forbid(unsafe_code)]

mod bundled;
mod model;
mod parser;

use model::{Book, Testament, Translation, BOOKS, DEFAULT_BOOK_INDEX};
use parser::parse_chapter_json;

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, RowLead, ScreenBuilder, Task,
    TaskId, TaskOutcome,
};
use kobo_ui::TextScale;
use std::process::ExitCode;

const MAX_TASK_BYTES: u32 = 256 * 1024;
const CHAPTERS_PER_PAGE: u32 = 40;
const TOTAL_BIBLE_CHAPTERS: u32 = 1189;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Reading,
    BookPicker,
    ChapterPicker,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Chapter {
        translation: Translation,
        book_index: usize,
        chapter: u32,
    },
    BookDownload {
        translation: Translation,
        book_index: usize,
        current_chapter: u32,
        total_chapters: u32,
    },
    EntireBibleDownload {
        translation: Translation,
        book_index: usize,
        current_chapter: u32,
        total_downloaded: u32,
    },
}

struct BibleApp {
    view: View,
    translation: Translation,
    book_index: usize,
    chapter: u32,
    page: usize,
    pages: Vec<Vec<String>>,
    text_scale: TextScale,
    current_raw_prose: String,

    // Book Picker
    testament_tab: usize, // 0 = OT, 1 = NT
    book_list_page: usize,

    // Chapter Picker
    picker_book_index: usize,
    chapter_picker_page: usize,

    // Async / Caching State
    awaiting: Option<Awaiting>,
    error_banner: Option<String>,
    download_status: Option<String>,
}

impl Default for BibleApp {
    fn default() -> Self {
        Self {
            view: View::Reading,
            translation: Translation::Bsb,
            book_index: DEFAULT_BOOK_INDEX, // Mark (0-indexed 40)
            chapter: 1,
            page: 0,
            pages: Vec::new(),
            text_scale: TextScale::ExtraLarge, // 140% large comfortable font scale on 300 PPI display
            current_raw_prose: String::new(),

            testament_tab: 1, // Default to New Testament tab in book picker
            book_list_page: 0,

            picker_book_index: DEFAULT_BOOK_INDEX,
            chapter_picker_page: 0,

            awaiting: None,
            error_banner: None,
            download_status: None,
        }
    }
}

impl KoboApp for BibleApp {
    fn on_start(&mut self, context: &mut Context) {
        self.load_chapter(context);
        self.show(context);
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Reading {
            if forward {
                if self.page + 1 < self.pages.len() {
                    self.page += 1;
                } else {
                    self.go_to_next_chapter(context);
                }
            } else {
                if self.page > 0 {
                    self.page -= 1;
                } else {
                    self.go_to_previous_chapter(context, true);
                }
            }
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.error_banner = None;

        // Reading Page Turns
        if action == action_id("page-prev") {
            if self.page > 0 {
                self.page -= 1;
            } else {
                self.go_to_previous_chapter(context, true);
            }
            self.show(context);
            return;
        }
        if action == action_id("page-next") {
            if self.page + 1 < self.pages.len() {
                self.page += 1;
            } else {
                self.go_to_next_chapter(context);
            }
            self.show(context);
            return;
        }

        // Direct Chapter Step
        if action == action_id("prev-chapter") {
            self.go_to_previous_chapter(context, false);
            self.show(context);
            return;
        }
        if action == action_id("next-chapter") {
            self.go_to_next_chapter(context);
            self.show(context);
            return;
        }

        // System Exits & Launcher Navigation
        if action == action_id("exit-to-reader") {
            context.exit();
            return;
        }
        if action == action_id("open-launcher") {
            context.launch("launcher");
            return;
        }

        // Navigation Views
        if action == action_id("open-books") {
            self.view = View::BookPicker;
            self.testament_tab = if self.book().testament == Testament::Old { 0 } else { 1 };
            self.book_list_page = 0;
            self.show(context);
            return;
        }
        if action == action_id("open-chapters") {
            self.picker_book_index = self.book_index;
            self.chapter_picker_page = 0;
            self.view = View::ChapterPicker;
            self.show(context);
            return;
        }
        if action == action_id("open-settings") {
            self.view = View::Settings;
            self.show(context);
            return;
        }
        if action == action_id("back-to-reading") {
            self.view = View::Reading;
            self.show(context);
            return;
        }
        if action == action_id("back-to-books") {
            self.view = View::BookPicker;
            self.show(context);
            return;
        }

        // Book Picker Tabs & Selection
        if action == action_id("tab-ot") {
            self.testament_tab = 0;
            self.book_list_page = 0;
            self.show(context);
            return;
        }
        if action == action_id("tab-nt") {
            self.testament_tab = 1;
            self.book_list_page = 0;
            self.show(context);
            return;
        }
        if action == action_id("books-prev") {
            if self.book_list_page > 0 {
                self.book_list_page -= 1;
            }
            self.show(context);
            return;
        }
        if action == action_id("books-next") {
            self.book_list_page += 1;
            self.show(context);
            return;
        }

        // Check for book selection actions: "pick-book-{idx}"
        for idx in 0..BOOKS.len() {
            if action == action_id(&format!("pick-book-{idx}")) {
                self.picker_book_index = idx;
                self.chapter_picker_page = 0;
                self.view = View::ChapterPicker;
                self.show(context);
                return;
            }
        }

        // Chapter Picker page turns
        if action == action_id("chap-grid-prev") {
            if self.chapter_picker_page > 0 {
                self.chapter_picker_page -= 1;
            }
            self.show(context);
            return;
        }
        if action == action_id("chap-grid-next") {
            self.chapter_picker_page += 1;
            self.show(context);
            return;
        }

        // Check for chapter selection actions: "pick-chap-{num}"
        let total_chaps = BOOKS[self.picker_book_index].chapters;
        for ch in 1..=total_chaps {
            if action == action_id(&format!("pick-chap-{ch}")) {
                self.book_index = self.picker_book_index;
                self.chapter = ch;
                self.page = 0;
                self.view = View::Reading;
                self.load_chapter(context);
                self.show(context);
                return;
            }
        }

        // Font Size Steppers & Chips in Settings
        if action == action_id("font-smaller") {
            self.step_font_scale(false, context);
            self.show(context);
            return;
        }
        if action == action_id("font-larger") {
            self.step_font_scale(true, context);
            self.show(context);
            return;
        }
        if action == action_id("size-100") {
            self.text_scale = TextScale::Default;
            self.repaginate(context);
            self.show(context);
            return;
        }
        if action == action_id("size-120") {
            self.text_scale = TextScale::Large;
            self.repaginate(context);
            self.show(context);
            return;
        }
        if action == action_id("size-140") {
            self.text_scale = TextScale::ExtraLarge;
            self.repaginate(context);
            self.show(context);
            return;
        }
        if action == action_id("size-160") {
            self.text_scale = TextScale::Huge;
            self.repaginate(context);
            self.show(context);
            return;
        }
        if action == action_id("size-180") {
            self.text_scale = TextScale::Largest;
            self.repaginate(context);
            self.show(context);
            return;
        }

        // Settings: Translation switch
        if action == action_id("trans-bsb") {
            self.translation = Translation::Bsb;
            self.page = 0;
            self.load_chapter(context);
            self.show(context);
            return;
        }
        if action == action_id("trans-web") {
            self.translation = Translation::Web;
            self.page = 0;
            self.load_chapter(context);
            self.show(context);
            return;
        }
        if action == action_id("trans-kjv") {
            self.translation = Translation::Kjv;
            self.page = 0;
            self.load_chapter(context);
            self.show(context);
            return;
        }

        // Settings: Downloads
        if action == action_id("download-book") {
            self.start_book_download(context);
            self.show(context);
            return;
        }
        if action == action_id("download-entire-bible") {
            self.start_entire_bible_download(context);
            self.show(context);
            return;
        }
        if action == action_id("cancel-download") {
            self.awaiting = None;
            self.download_status = Some("Download cancelled.".into());
            self.show(context);
            return;
        }

        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, _task_id: TaskId, outcome: TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(bytes) => {
                if let Ok(json_str) = std::str::from_utf8(&bytes) {
                    if let Some(awaiting) = self.awaiting {
                        match awaiting {
                            Awaiting::Chapter {
                                translation,
                                book_index,
                                chapter,
                            } => {
                                let cache_key = format!(
                                    "c:{}:{}:{}",
                                    translation.id(),
                                    BOOKS[book_index].id,
                                    chapter
                                );
                                context.store().save(&cache_key, json_str);

                                if translation == self.translation
                                    && book_index == self.book_index
                                    && chapter == self.chapter
                                {
                                    if let Ok(parsed) = parse_chapter_json(json_str) {
                                        self.current_raw_prose = parsed.formatted_prose;
                                        self.repaginate(context);
                                    }
                                }
                                self.awaiting = None;
                            }
                            Awaiting::BookDownload {
                                translation,
                                book_index,
                                current_chapter,
                                total_chapters,
                            } => {
                                let cache_key = format!(
                                    "c:{}:{}:{}",
                                    translation.id(),
                                    BOOKS[book_index].id,
                                    current_chapter
                                );
                                context.store().save(&cache_key, json_str);

                                if current_chapter < total_chapters {
                                    let next_ch = current_chapter + 1;
                                    self.download_status = Some(format!(
                                        "Downloading {} (Chapter {}/{})...",
                                        BOOKS[book_index].name, next_ch, total_chapters
                                    ));
                                    self.awaiting = Some(Awaiting::BookDownload {
                                        translation,
                                        book_index,
                                        current_chapter: next_ch,
                                        total_chapters,
                                    });
                                    self.spawn_fetch(
                                        context,
                                        translation,
                                        BOOKS[book_index].id,
                                        next_ch,
                                    );
                                } else {
                                    self.awaiting = None;
                                    self.download_status = Some(format!(
                                        "✓ Complete book of {} saved for offline reading!",
                                        BOOKS[book_index].name
                                    ));
                                }
                            }
                            Awaiting::EntireBibleDownload {
                                translation,
                                book_index,
                                current_chapter,
                                total_downloaded,
                            } => {
                                let cache_key = format!(
                                    "c:{}:{}:{}",
                                    translation.id(),
                                    BOOKS[book_index].id,
                                    current_chapter
                                );
                                context.store().save(&cache_key, json_str);

                                let new_total = total_downloaded + 1;
                                let percent = (new_total * 100) / TOTAL_BIBLE_CHAPTERS;

                                let (next_b_idx, next_ch) = if current_chapter < BOOKS[book_index].chapters {
                                    (book_index, current_chapter + 1)
                                } else if book_index + 1 < BOOKS.len() {
                                    (book_index + 1, 1)
                                } else {
                                    (BOOKS.len(), 0)
                                };

                                if next_b_idx < BOOKS.len() {
                                    self.download_status = Some(format!(
                                        "Downloading Bible: {} {} ({}/1189 ch, {}%)...",
                                        BOOKS[next_b_idx].name, next_ch, new_total, percent
                                    ));
                                    self.awaiting = Some(Awaiting::EntireBibleDownload {
                                        translation,
                                        book_index: next_b_idx,
                                        current_chapter: next_ch,
                                        total_downloaded: new_total,
                                    });
                                    self.spawn_fetch(
                                        context,
                                        translation,
                                        BOOKS[next_b_idx].id,
                                        next_ch,
                                    );
                                } else {
                                    self.awaiting = None;
                                    self.download_status = Some(
                                        "✓ Entire Holy Bible (1,189 chapters) downloaded for offline reading!".into(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            TaskOutcome::Failed(_err) => {
                self.awaiting = None;
                self.error_banner =
                    Some("Could not fetch passage. Connect Wi-Fi or read bundled books.".into());
            }
            TaskOutcome::Cancelled => {
                self.awaiting = None;
            }
        }
        self.show(context);
    }
}

impl BibleApp {
    fn book(&self) -> &'static Book {
        &BOOKS[self.book_index]
    }

    fn next_book_ref(&self) -> Option<(&'static Book, u32)> {
        if self.chapter < self.book().chapters {
            Some((self.book(), self.chapter + 1))
        } else if self.book_index + 1 < BOOKS.len() {
            Some((&BOOKS[self.book_index + 1], 1))
        } else {
            None
        }
    }

    fn prev_book_ref(&self) -> Option<(&'static Book, u32)> {
        if self.chapter > 1 {
            Some((self.book(), self.chapter - 1))
        } else if self.book_index > 0 {
            let prev_b = &BOOKS[self.book_index - 1];
            Some((prev_b, prev_b.chapters))
        } else {
            None
        }
    }

    fn load_chapter(&mut self, context: &mut Context) {
        let trans = self.translation.id();
        let book_id = self.book().id;
        let chap = self.chapter;

        // 1. Check bundled offline assets
        if let Some(json_str) = bundled::get_bundled_json(trans, book_id, chap) {
            if let Ok(parsed) = parse_chapter_json(json_str) {
                self.current_raw_prose = parsed.formatted_prose;
                self.repaginate(context);
                return;
            }
        }

        // 2. Otherwise initiate background fetch
        self.awaiting = Some(Awaiting::Chapter {
            translation: self.translation,
            book_index: self.book_index,
            chapter: self.chapter,
        });
        self.current_raw_prose = format!(
            "Loading {} {} ({})...",
            self.book().name,
            self.chapter,
            self.translation
        );
        self.repaginate(context);
        self.spawn_fetch(context, self.translation, book_id, chap);
    }

    fn spawn_fetch(
        &mut self,
        context: &mut Context,
        trans: Translation,
        book_id: &str,
        chapter: u32,
    ) {
        let url = format!(
            "https://bible.helloao.org/api/{}/{}/{}.simple.json",
            trans.id(),
            book_id,
            chapter
        );
        context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_TASK_BYTES,
            credential: None,
            headers: Vec::new(),
        });
    }

    fn repaginate(&mut self, context: &Context) {
        if self.current_raw_prose.is_empty() {
            self.pages = vec![vec!["No text available.".to_string()]];
            self.page = 0;
            return;
        }

        let pages = context.paginate_at(&self.current_raw_prose, true, self.text_scale);
        if pages.is_empty() {
            self.pages = vec![vec![self.current_raw_prose.clone()]];
        } else {
            self.pages = pages;
        }
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }

    fn step_font_scale(&mut self, increase: bool, context: &Context) {
        let steps = TextScale::STEPS;
        let current_pos = steps.iter().position(|&s| s == self.text_scale).unwrap_or(6);
        if increase && current_pos + 1 < steps.len() {
            self.text_scale = steps[current_pos + 1];
        } else if !increase && current_pos > 0 {
            self.text_scale = steps[current_pos - 1];
        }
        self.repaginate(context);
    }

    fn go_to_previous_chapter(&mut self, context: &mut Context, to_last_page: bool) {
        if let Some((_, prev_ch)) = self.prev_book_ref() {
            if self.chapter > 1 {
                self.chapter = prev_ch;
            } else {
                self.book_index -= 1;
                self.chapter = prev_ch;
            }
            self.load_chapter(context);
            if to_last_page {
                self.page = self.pages.len().saturating_sub(1);
            } else {
                self.page = 0;
            }
        }
    }

    fn go_to_next_chapter(&mut self, context: &mut Context) {
        if let Some((_, next_ch)) = self.next_book_ref() {
            if self.chapter < self.book().chapters {
                self.chapter = next_ch;
            } else {
                self.book_index += 1;
                self.chapter = next_ch;
            }
            self.page = 0;
            self.load_chapter(context);
        }
    }

    fn start_book_download(&mut self, context: &mut Context) {
        let total = self.book().chapters;
        self.download_status = Some(format!(
            "Downloading {} 1 of {}...",
            self.book().name,
            total
        ));
        self.awaiting = Some(Awaiting::BookDownload {
            translation: self.translation,
            book_index: self.book_index,
            current_chapter: 1,
            total_chapters: total,
        });
        self.spawn_fetch(context, self.translation, self.book().id, 1);
    }

    fn start_entire_bible_download(&mut self, context: &mut Context) {
        self.download_status = Some(format!(
            "Downloading Bible: Genesis 1 (1/{TOTAL_BIBLE_CHAPTERS} chapters)..."
        ));
        self.awaiting = Some(Awaiting::EntireBibleDownload {
            translation: self.translation,
            book_index: 0,
            current_chapter: 1,
            total_downloaded: 0,
        });
        self.spawn_fetch(context, self.translation, BOOKS[0].id, 1);
    }

    fn show(&self, context: &mut Context) {
        match self.view {
            View::Reading => self.show_reading(context),
            View::BookPicker => self.show_book_picker(context),
            View::ChapterPicker => self.show_chapter_picker(context),
            View::Settings => self.show_settings(context),
        }
    }

    fn show_reading(&self, context: &mut Context) {
        let title = format!("{} {} · {}", self.book().name, self.chapter, self.translation);
        // Note: exactly 2 top bar actions so Cobalt's renderer never clips them!
        let mut builder = ScreenBuilder::new("reading")
            .top_bar(title)
            .top_bar_action("open-books", "Books")
            .top_bar_action("open-settings", "Aa / Settings")
            .reading(true)
            .text_scale(self.text_scale);

        if let Some(err) = &self.error_banner {
            builder = builder.banner(BannerLevel::Attention, err);
        }

        if self.pages.is_empty() {
            builder = builder.skeleton(5);
        } else {
            let page_idx = self.page.min(self.pages.len().saturating_sub(1));
            let is_last_page = page_idx + 1 == self.pages.len();

            for paragraph in &self.pages[page_idx] {
                builder = builder.text(paragraph);
            }

            // If on the last page of a chapter, provide a clear Next Chapter prompt button
            if is_last_page {
                if let Some((next_book, next_ch)) = self.next_book_ref() {
                    builder = builder.primary_button(
                        "next-chapter",
                        format!("Next: {} {} →", next_book.name, next_ch),
                    );
                }
            }

            builder = builder
                .page_turns("page-prev", "page-next")
                .page_position(
                    u16::try_from(page_idx + 1).unwrap_or(u16::MAX),
                    u16::try_from(self.pages.len()).unwrap_or(u16::MAX),
                );
        }

        context.set_screen(builder.build());
    }

    fn show_book_picker(&self, context: &mut Context) {
        let mut builder = ScreenBuilder::new("book-picker")
            .top_bar("Holy Bible — Select Book")
            .top_bar_action("back-to-reading", "Reading")
            .top_bar_action("exit-to-reader", "Exit to Kobo")
            .tabs(
                self.testament_tab,
                [("tab-ot", "Old Testament (39)"), ("tab-nt", "New Testament (27)")],
            );

        let target_testament = if self.testament_tab == 0 {
            Testament::Old
        } else {
            Testament::New
        };

        let books: Vec<(usize, &'static Book)> = BOOKS
            .iter()
            .enumerate()
            .filter(|(_, b)| b.testament == target_testament)
            .collect();

        // 7 rows per page fits comfortably
        let per_page = 7;
        let total_pages = (books.len() + per_page - 1) / per_page;
        let page = self.book_list_page.min(total_pages.saturating_sub(1));

        let start = page * per_page;
        let end = (start + per_page).min(books.len());
        let slice = &books[start..end];

        let rows = slice.iter().map(|(orig_idx, b)| {
            (
                format!("pick-book-{}", orig_idx),
                b.name.to_string(),
                format!("{} chapters", b.chapters),
                RowLead::Icon(Glyph::Book),
            )
        });

        builder = builder.rows(rows);

        if total_pages > 1 {
            builder = builder
                .page_turns("books-prev", "books-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(total_pages).unwrap_or(u16::MAX),
                );
        }

        context.set_screen(builder.build());
    }

    fn show_chapter_picker(&self, context: &mut Context) {
        let book = &BOOKS[self.picker_book_index];
        let total_chaps = book.chapters;

        let total_pages = (total_chaps + CHAPTERS_PER_PAGE - 1) / CHAPTERS_PER_PAGE;
        let page = self.chapter_picker_page.min(total_pages.saturating_sub(1) as usize);

        let start_ch = (page as u32) * CHAPTERS_PER_PAGE + 1;
        let end_ch = (start_ch + CHAPTERS_PER_PAGE - 1).min(total_chaps);

        let mut builder = ScreenBuilder::new("chapter-picker")
            .top_bar(format!("{} — Select Chapter", book.name))
            .top_bar_action("back-to-books", "Books")
            .top_bar_action("back-to-reading", "Reading");

        // 5 columns of square buttons
        let cells = (start_ch..=end_ch)
            .map(|ch| (format!("pick-chap-{}", ch), format!("{}", ch)));

        builder = builder.grid(5, false, cells);

        if total_pages > 1 {
            builder = builder
                .page_turns("chap-grid-prev", "chap-grid-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(total_pages).unwrap_or(u16::MAX),
                );
        }

        context.set_screen(builder.build());
    }

    fn show_settings(&self, context: &mut Context) {
        let mut builder = ScreenBuilder::new("settings")
            .top_bar("Bible Settings")
            .top_bar_action("back-to-reading", "Done")
            .top_bar_action("exit-to-reader", "Exit to Kobo");

        if let Some(msg) = &self.download_status {
            builder = builder.banner(BannerLevel::Info, msg);
        }

        // Font Size Section
        let scale_percent = self.text_scale.percent();
        let scale_label = format!("Text Scale: {}%", scale_percent);
        let steps = TextScale::STEPS;
        let current_pos = steps.iter().position(|&s| s == self.text_scale).unwrap_or(6);
        let can_less = current_pos > 0;
        let can_more = current_pos + 1 < steps.len();

        builder = builder
            .heading("Font Size & Typography")
            .stepper(
                scale_label,
                "font-smaller",
                Glyph::Minus,
                "font-larger",
                Glyph::Plus,
            )
            .stepper_ends(can_less, can_more)
            .chips([
                ("size-100", "100%", self.text_scale == TextScale::Default),
                ("size-120", "120%", self.text_scale == TextScale::Large),
                ("size-140", "140%", self.text_scale == TextScale::ExtraLarge),
                ("size-160", "160%", self.text_scale == TextScale::Huge),
                ("size-180", "180%", self.text_scale == TextScale::Largest),
            ]);

        // Translation Selector
        builder = builder
            .heading("Translation")
            .chips([
                ("trans-bsb", "BSB (Berean)", self.translation == Translation::Bsb),
                ("trans-web", "WEB (World English)", self.translation == Translation::Web),
                ("trans-kjv", "KJV (King James)", self.translation == Translation::Kjv),
            ]);

        // Offline Download Section
        let book_name = self.book().name;
        builder = builder
            .heading("Offline Storage")
            .primary_button(
                "download-entire-bible",
                "Download Entire Bible (All 66 Books) Offline",
            )
            .button(
                "download-book",
                format!("Download All Chapters of {} Offline", book_name),
            );

        if self.awaiting.is_some() {
            builder = builder.button("cancel-download", "Cancel Active Download");
        }

        // App Exit Section
        builder = builder
            .heading("System & Navigation")
            .primary_button("exit-to-reader", "Exit to Stock Kobo Home Screen")
            .button("open-launcher", "Switch to Cobalt Launcher Grid");

        context.set_screen(builder.build());
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("bible", BibleApp::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command};

    #[test]
    fn app_starts_on_mark_1_with_bsb() {
        let runner = AppRunner::new(BibleApp::default());
        let app = runner.app();
        assert_eq!(app.translation, Translation::Bsb);
        assert_eq!(app.book().id, "MRK");
        assert_eq!(app.chapter, 1);
        assert_eq!(app.view, View::Reading);
        assert_eq!(app.text_scale, TextScale::ExtraLarge);
    }

    #[test]
    fn bundled_mark_1_loads_offline() {
        let json = bundled::get_bundled_json("BSB", "MRK", 1).expect("Mark 1 must be bundled");
        let parsed = parse_chapter_json(json).expect("Mark 1 must parse cleanly");
        assert_eq!(parsed.book_id, "MRK");
        assert_eq!(parsed.chapter, 1);
        assert!(parsed.verse_count >= 45);
        assert!(parsed.formatted_prose.contains("beginning of the gospel"));
    }

    #[test]
    fn settings_navigation_and_exit_commands() {
        let mut runner = AppRunner::new(BibleApp::default());
        runner.start();

        // 1. Navigate to Settings
        runner.action(action_id("open-settings"));
        assert_eq!(runner.app().view, View::Settings);

        // 2. Adjust font size via chips
        runner.action(action_id("size-180"));
        assert_eq!(runner.app().text_scale, TextScale::Largest);
        runner.action(action_id("size-140"));
        assert_eq!(runner.app().text_scale, TextScale::ExtraLarge);

        // 3. Test Steppers
        runner.action(action_id("font-smaller"));
        assert_eq!(runner.app().text_scale, TextScale::Larger);
        runner.action(action_id("font-larger"));
        assert_eq!(runner.app().text_scale, TextScale::ExtraLarge);

        // 4. Test Translation Switch
        runner.action(action_id("trans-web"));
        assert_eq!(runner.app().translation, Translation::Web);

        // 5. Test Return to Reading
        runner.action(action_id("back-to-reading"));
        assert_eq!(runner.app().view, View::Reading);

        // 6. Test Exit to Reader Command
        runner.action(action_id("open-settings"));
        let commands = runner.action(action_id("exit-to-reader"));
        assert!(commands.iter().any(|c| matches!(c, Command::Exit)));
    }

    #[test]
    fn launcher_switch_command() {
        let mut runner = AppRunner::new(BibleApp::default());
        runner.start();
        runner.action(action_id("open-settings"));
        let commands = runner.action(action_id("open-launcher"));
        assert!(commands.iter().any(|c| matches!(c, Command::Launch(name) if name == "launcher")));
    }

    #[test]
    fn books_and_chapter_picker_flow() {
        let mut runner = AppRunner::new(BibleApp::default());
        runner.start();

        runner.action(action_id("open-books"));
        assert_eq!(runner.app().view, View::BookPicker);

        // Select Genesis (index 0)
        runner.action(action_id("pick-book-0"));
        assert_eq!(runner.app().view, View::ChapterPicker);
        assert_eq!(runner.app().picker_book_index, 0);

        // Select Genesis 3
        runner.action(action_id("pick-chap-3"));
        assert_eq!(runner.app().view, View::Reading);
        assert_eq!(runner.app().book().id, "GEN");
        assert_eq!(runner.app().chapter, 3);
    }

    #[test]
    fn chapter_boundary_and_hardware_buttons() {
        let mut runner = AppRunner::new(BibleApp::default());
        runner.start();
        assert_eq!(runner.app().chapter, 1);

        // Hardware Page turn forward at end of chapter
        runner.app_mut().page = runner.app().pages.len().saturating_sub(1);
        runner.page_turn(true);
        assert_eq!(runner.app().chapter, 2);

        // Hardware Page turn backward at start of chapter
        runner.app_mut().page = 0;
        runner.page_turn(false);
        assert_eq!(runner.app().chapter, 1);
    }
}
