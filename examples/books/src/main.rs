//! The bundled library: documents already on this device.
//!
//! Books is an included application. It lists files on the card (`/mnt/onboard`,
//! `/mnt/sd`, Cobalt shelves) and titles from the stock Kobo library. EPUB,
//! Markdown, HTML and plain text open in the shared reader, at the page and
//! text size they were closed at. PDF and Kobo Store books are listed after
//! them and say they open in the Kobo reader. A Kobo Store title that has not
//! been downloaded is listed and says so when opened.

use kobo_bookview::positions::{Progress, KEY as READING};
use kobo_bookview::{BookView, Step};
use kobo_read::Outcome;
use kobo_sdk::{
    action_id, document_preview, stamp_format_badge, ActionId, BannerLevel, Context, DeviceRequest,
    DeviceResult, Glyph, KoboApp, LibraryEntry, PictureHandle, Screen, ScreenBuilder, StoreResult,
    TaskId, TaskOutcome, Tile, TilePicture, TileShape,
};
use std::process::ExitCode;

/// How many books one shelf page holds.
///
/// Three columns of portrait tiles, two rows deep, matching gutenbird: that
/// is what fits whole between the bars on this panel.
#[cfg(test)]
const SHELF_PAGE: usize = 6;

const PREVIOUS: &str = "previous";
const NEXT: &str = "next";
const BACK: &str = "back";
const RELIST: &str = "relist";
const RETRY_SAVE: &str = "retry-save";

/// Where to send an owner for anything Books cannot open itself.
const IN_KOBO_READER: &str = "To get there, tap Kobo reader at the foot of the Cobalt home screen.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Epub,
    Markdown,
    Html,
    Text,
    Pdf,
    /// A Kobo Store book: listed with its real title, refused on a tap.
    Kepub,
}

impl Kind {
    const fn badge(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Markdown => "md",
            Self::Html => "html",
            Self::Text => "txt",
            Self::Pdf => "pdf",
            Self::Kepub => "kepub",
        }
    }

    const fn is_readable(self) -> bool {
        !matches!(self, Self::Pdf | Self::Kepub)
    }

    const fn filename(self) -> &'static str {
        match self {
            Self::Epub => "open-sea.epub",
            Self::Markdown => "notes.md",
            Self::Html => "streets.html",
            Self::Text => "letter.txt",
            Self::Pdf => "timetable.pdf",
            Self::Kepub => "purchase.kepub.epub",
        }
    }
}

impl Document {
    /// Whether tapping it opens it here rather than sending the owner to the
    /// Kobo reader.
    const fn opens_here(&self) -> bool {
        self.on_card && self.kind.is_readable()
    }
}

#[derive(Clone, Debug)]
struct Document {
    id: String,
    title: String,
    kind: Kind,
    /// Opening words, used as the tile when there is no jacket.
    preview: String,
    on_card: bool,
    has_cover: bool,
    /// Present only for in-process tests that never talk to the runtime.
    body: Option<String>,
}

#[cfg(test)]
fn fixtures() -> Vec<Document> {
    vec![
        Document {
            id: "test/open-sea.epub".to_owned(),
            title: "The Open Sea".to_owned(),
            kind: Kind::Epub,
            preview: "The tide was already turning when the boat left the harbour.".to_owned(),
            on_card: true,
            has_cover: true,
            body: Some("The tide was already turning when the boat left the harbour.".to_owned()),
        },
        Document {
            id: "test/notes.md".to_owned(),
            title: "Notes on a Rainy Afternoon".to_owned(),
            kind: Kind::Markdown,
            preview: "The rain started before the kettle boiled.".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some(
                "# Notes on a Rainy Afternoon\n\nThe rain started before the kettle boiled."
                    .to_owned(),
            ),
        },
        Document {
            id: "test/timetable.pdf".to_owned(),
            title: "Timetable".to_owned(),
            kind: Kind::Pdf,
            preview: "Northbound 06:12".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some(String::new()),
        },
        Document {
            id: "test/letter.txt".to_owned(),
            title: "A Letter Home".to_owned(),
            kind: Kind::Text,
            preview: "Dear M,".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some(
                "Dear M,\n\nThe hill behind the house is still green in September.".to_owned(),
            ),
        },
        Document {
            id: "test/streets.html".to_owned(),
            title: "Index of Streets".to_owned(),
            kind: Kind::Html,
            preview: "Market Street runs east from the harbour.".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some(
                "<h1>Index of Streets</h1>\n<p>Market Street runs east from the harbour.</p>"
                    .to_owned(),
            ),
        },
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Shelf,
    Opening(usize),
    Reading,
    Unreadable(usize),
}

struct Books {
    view: View,
    page: usize,
    documents: Vec<Document>,
    book: BookView,
    open_title: Option<String>,
    /// Where every book was left, and at what size.
    ///
    /// Every open used to start from a blank record and nothing was ever
    /// written back, so closing a book lost the page, the text size, the
    /// bookmarks and the highlights. The shared record is the one Feeds and
    /// the other readers keep, which also gives a book opened for the first
    /// time the text size last chosen here.
    progress: Progress,
    /// The record the open book is kept under.
    reading_id: Option<String>,
    /// Whether the last attempt to list the library failed, which is not the
    /// same thing as a library with nothing in it.
    library_failed: bool,
}

impl Default for Books {
    fn default() -> Self {
        Self::empty()
    }
}

impl Books {
    #[cfg(test)]
    fn seeded() -> Self {
        let mut books = Self::empty();
        books.take_documents(fixtures());
        books
    }

    fn empty() -> Self {
        Self {
            view: View::Shelf,
            page: 0,
            documents: Vec::new(),
            book: BookView::new(),
            open_title: None,
            progress: Progress::default(),
            reading_id: None,
            library_failed: false,
        }
    }

    /// Puts what can be read here first, so the first page of the shelf is
    /// books that open rather than PDFs and Store titles that send the owner
    /// somewhere else. The runtime's own order is kept within each group.
    fn take_documents(&mut self, mut documents: Vec<Document>) {
        documents.sort_by_key(|document| !document.opens_here());
        self.documents = documents;
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Shelf | View::Opening(_) => self.shelf(context),
            View::Reading => self.reading(),
            View::Unreadable(index) => self.unreadable(index),
        };
        context.set_screen(screen);
    }

    fn shelf(&mut self, context: &mut Context) -> Screen {
        let mut screen = ScreenBuilder::new("books").top_bar("Books");
        if self.library_failed && self.documents.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Book),
                    "The library could not be read",
                    "Books could not list the files on this reader. Try again.",
                )
                .button(RELIST, "Try again")
                .build();
        }
        if self.documents.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Book),
                    "No books yet",
                    "Plug the reader into a computer and copy EPUB, Markdown, HTML or text files onto it. They appear here the next time you open Books.",
                )
                .build();
        }
        screen = self.place_notice(screen);
        let pages = self.pages(context);
        self.page = self.page.min(pages.len().saturating_sub(1));
        let page = pages.get(self.page).cloned().unwrap_or_default();
        let (width, height) = tile_pixels(context);
        let tiles = page.into_iter().filter_map(|index| {
            let document = self.documents.get(index)?;
            let title = document.title.clone();
            let elsewhere = !document.opens_here();
            let picture = self.picture(context, index, width, height);
            Some((
                format!("book-{index}"),
                title,
                Glyph::Book,
                move |blank: Tile| {
                    let drawn = match picture {
                        Some(picture) => blank.with_picture(picture),
                        None => blank,
                    };
                    if elsewhere {
                        drawn.with_subtitle("Opens in Kobo reader")
                    } else {
                        drawn
                    }
                },
            ))
        });
        screen = screen.tile_grid(TileShape::Portrait, tiles);
        let page_count = pages.len().max(1);
        if page_count > 1 {
            let page = u16::try_from(self.page + 1).unwrap_or(u16::MAX);
            let total = u16::try_from(page_count).unwrap_or(u16::MAX);
            screen = screen.page_turns(PREVIOUS, NEXT).page_position(page, total);
        }
        screen.build()
    }

    fn reading(&self) -> Screen {
        let title = self.open_title.as_deref().unwrap_or("Reading");
        self.book.screen(title).unwrap_or_else(Self::missing_reader)
    }

    fn missing_reader() -> Screen {
        ScreenBuilder::new("books-unreadable")
            .top_bar("Books")
            .splash(
                Some(Glyph::Book),
                "This document",
                "This file could not be opened. It may be damaged, or written in a way Books cannot read.",
            )
            .button(BACK, "Back")
            .build()
    }

    /// Says where a book that does not open here can be read instead.
    ///
    /// "Not readable yet" left the owner holding a book with nowhere to take
    /// it. Each of these does open in the stock reader, so the screen says
    /// that, and how to get there.
    fn unreadable(&self, index: usize) -> Screen {
        let document = self.documents.get(index);
        let title = document.map_or("This document", |document| document.title.as_str());
        let reason = if document.is_some_and(|document| !document.on_card) {
            format!("This book is in your Kobo library but is not downloaded yet. Open it in the Kobo reader to download it. {IN_KOBO_READER}")
        } else if document.is_some_and(|document| document.kind == Kind::Kepub) {
            format!("Books from the Kobo Store open in the Kobo reader. {IN_KOBO_READER}")
        } else if document.is_some_and(|document| document.kind == Kind::Pdf) {
            format!(
                "PDFs open in the Kobo reader. Books cannot show PDF pages yet. {IN_KOBO_READER}"
            )
        } else {
            "This file could not be opened. It may be damaged, or written in a way Books cannot read."
                .to_owned()
        };
        // The bar's own back arrow returns to the shelf too. Without owning
        // it, that arrow left Books altogether from a screen that is only a
        // note about one book.
        ScreenBuilder::new("books-unreadable")
            .top_bar("Books")
            .owns_back(true)
            .splash(Some(Glyph::Book), title, reason)
            .build()
    }

    /// Says on the shelf that the last place was not kept, with the fix on
    /// the same banner.
    ///
    /// On the shelf no book is open, so nothing would try the write again
    /// until a book was opened and paged; Retry saving does it now. A full
    /// record of places is not mended by trying again, so that one is said
    /// without the control.
    fn place_notice(&self, screen: ScreenBuilder) -> ScreenBuilder {
        if !self.progress.failed {
            return screen;
        }
        if self.progress.out_of_room() {
            return screen.banner(
                BannerLevel::Error,
                "Books has no room left to save your place in another book.",
            );
        }
        screen
            .banner(
                BannerLevel::Error,
                "Books could not save your place in the last book.",
            )
            .banner_action(RETRY_SAVE, "Retry saving")
    }

    fn pages(&self, context: &Context) -> Vec<Vec<usize>> {
        // Measured under the banner, so a notice with its action line takes
        // room from the grid's page rather than squeezing every tile.
        let placed = self
            .place_notice(ScreenBuilder::new("books").top_bar("Books"))
            .build();
        let pages =
            context.paginate_tiles_under(self.documents.len(), TileShape::Portrait, false, &placed);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn picture(
        &self,
        context: &mut Context,
        index: usize,
        width: u32,
        height: u32,
    ) -> Option<TilePicture> {
        let document = self.documents.get(index)?;
        let mut grey = if document.has_cover {
            painted_cover(index, width, height)
        } else {
            document_preview(&document.preview, document.kind.badge(), width, height)
        };
        if document.has_cover {
            stamp_format_badge(&mut grey, width, height, document.kind.badge());
        }
        context.put_picture(
            PictureHandle(u32::try_from(index).unwrap_or(0)),
            width,
            height,
            grey,
        )
    }

    fn open_document(&mut self, context: &mut Context, index: usize) {
        let Some(document) = self.documents.get(index).cloned() else {
            return;
        };
        if !document.opens_here() {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
        }
        if let Some(body) = document.body.as_ref() {
            let bytes = fixture_bytes(&document, body);
            self.finish_open(context, index, &document, &bytes);
            return;
        }
        self.view = View::Opening(index);
        self.show(context);
        context.device().read_library(document.id);
    }

    fn finish_open(
        &mut self,
        context: &mut Context,
        index: usize,
        document: &Document,
        bytes: &[u8],
    ) {
        if bytes.is_empty() {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
        }
        let id = position_id(&document.id);
        let memory = self.progress.memory(&id);
        if self
            .book
            .open_bytes(context, document.kind.filename(), bytes, memory)
            .is_ok()
        {
            self.open_title = Some(document.title.clone());
            self.reading_id = Some(id);
            self.view = View::Reading;
        } else {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
        }
        self.show(context);
    }

    fn close_reader(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.book.close(context);
        self.open_title = None;
        self.reading_id = None;
        self.view = View::Shelf;
        self.show(context);
    }

    /// Writes down where the open book is, at what size, with its marks.
    ///
    /// A record that failed to save is tried again on the next page turn
    /// rather than given up on, because the usual cause is a write that
    /// collided with something else and will go through the next time.
    fn keep_position(&mut self, context: &mut Context) {
        let (Some(id), Some(memory)) = (&self.reading_id, self.book.memory()) else {
            return;
        };
        self.progress.keep(context, id.clone(), memory.clone());
        if self.progress.failed {
            self.progress.retry(context);
        }
    }

    fn read_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if !matches!(self.view, View::Reading) {
            return false;
        }
        if let Some(outcome) = self.book.act(context, action) {
            match outcome {
                Outcome::Close => self.close_reader(context),
                Outcome::Light(level) => {
                    context.device().set_frontlight(level);
                    self.keep_position(context);
                    self.show(context);
                }
                Outcome::Save => {
                    self.keep_position(context);
                    self.show(context);
                }
                Outcome::Elsewhere | Outcome::Repaint => self.show(context),
            }
            return true;
        }
        if action == ActionId::BACK || action == action_id(BACK) {
            self.close_reader(context);
            return true;
        }
        false
    }
}

impl KoboApp for Books {
    /// The platform's own blue: this screen is Cobalt itself rather than an
    /// application running on it, and it wears the colour of the logo.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Cobalt)
    }

    fn on_start(&mut self, context: &mut Context) {
        context.store().load(READING);
        if self.documents.is_empty() {
            context.device().list_library();
        }
        self.show(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == READING {
            self.progress.load(result);
            self.show(context);
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == READING {
            self.progress.saved(context, result);
            if matches!(self.view, View::Shelf) {
                self.show(context);
            }
        }
    }

    /// Lists the card again when the shelf comes back into view, so a book
    /// copied on while Books was in the background is there without a
    /// restart.
    fn on_foreground(&mut self, context: &mut Context) {
        if matches!(self.view, View::Shelf) {
            context.device().list_library();
        }
    }

    /// Keeps the place before the panel goes elsewhere, because an
    /// application in the background can be stopped without another word.
    fn on_background(&mut self, context: &mut Context) {
        self.keep_position(context);
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.keep_position(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.keep_position(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.read_action(context, action) {
            return;
        }
        if action == action_id(RELIST) {
            self.library_failed = false;
            context.device().list_library();
            return;
        }
        if action == action_id(RETRY_SAVE) {
            self.progress.retry(context);
            self.show(context);
            return;
        }
        if action == action_id(BACK)
            || (action == ActionId::BACK && matches!(self.view, View::Unreadable(_)))
        {
            self.view = View::Shelf;
            self.show(context);
            return;
        }
        if matches!(self.view, View::Shelf)
            && (action == action_id(NEXT) || action == action_id(PREVIOUS))
        {
            let pages = self.pages(context).len().max(1);
            self.page = if action == action_id(NEXT) {
                (self.page + 1).min(pages - 1)
            } else {
                self.page.saturating_sub(1)
            };
            self.show(context);
            return;
        }
        if let Some(index) =
            (0..self.documents.len()).find(|index| action == action_id(&format!("book-{index}")))
        {
            self.open_document(context, index);
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        match self.book.woke(context, task, &outcome) {
            Step::Repaint => self.show(context),
            Step::Quiet | Step::Elsewhere => {}
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if request == DeviceRequest::ReadFrontlight {
            if let DeviceResult::Frontlight { percent } = result {
                if self.book.took_light(percent) {
                    self.show(context);
                }
            }
            return;
        }
        if request == DeviceRequest::ListLibrary {
            match result {
                DeviceResult::Library { entries, .. } => {
                    self.library_failed = false;
                    self.take_documents(entries.into_iter().filter_map(from_entry).collect());
                    if matches!(self.view, View::Shelf) {
                        self.show(context);
                    }
                }
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                    self.library_failed = true;
                    if matches!(self.view, View::Shelf) {
                        self.show(context);
                    }
                }
                _ => {}
            }
            return;
        }
        if let DeviceRequest::ReadLibrary { .. } = request {
            match result {
                DeviceResult::LibraryDocument { bytes, .. } => {
                    if let View::Opening(index) = self.view {
                        if let Some(document) = self.documents.get(index).cloned() {
                            self.finish_open(context, index, &document, &bytes);
                        }
                    }
                }
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                    if let View::Opening(index) = self.view {
                        self.book.close(context);
                        self.open_title = None;
                        self.view = View::Unreadable(index);
                        self.show(context);
                    }
                }
                _ => {}
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if matches!(self.view, View::Reading) {
            let action = if forward {
                kobo_read::action::FORWARD
            } else {
                kobo_read::action::BACK
            };
            let _ = self.read_action(context, action_id(action));
        }
    }
}

/// The record a book's place is kept under.
///
/// The shared record names an article by the SHA-256 of something that
/// identifies it, and only that, so a library path never reaches the file.
/// The prefix keeps a book from ever sharing a record with anything else
/// that might one day be kept in the same file.
fn position_id(document: &str) -> String {
    kobo_bookview::positions::identity(&format!("books:{document}"))
}

fn from_entry(entry: LibraryEntry) -> Option<Document> {
    let kind = match entry.kind {
        1 => Kind::Epub,
        2 => Kind::Markdown,
        3 => Kind::Html,
        4 => Kind::Text,
        5 => Kind::Pdf,
        6 => Kind::Kepub,
        _ => return None,
    };
    Some(Document {
        id: entry.id,
        preview: entry.title.clone(),
        title: entry.title,
        kind,
        on_card: entry.on_card,
        has_cover: matches!(kind, Kind::Epub) && entry.on_card,
        body: None,
    })
}

fn fixture_bytes(document: &Document, body: &str) -> Vec<u8> {
    match document.kind {
        Kind::Epub => kobo_doc::epub::write(
            &document.title,
            Some("A. Mariner"),
            &[kobo_doc::epub::Chapter {
                title: "Leaving harbour".to_owned(),
                body: body.to_owned(),
            }],
        )
        .unwrap_or_default(),
        // Neither is handed to a reader, so neither needs bytes.
        Kind::Pdf | Kind::Kepub => Vec::new(),
        Kind::Markdown | Kind::Html | Kind::Text => body.as_bytes().to_vec(),
    }
}

fn tile_pixels(context: &Context) -> (u32, u32) {
    let (width, height) = context.metrics().tile_body(TileShape::Portrait);
    (
        u32::try_from(width.max(1)).unwrap_or(1),
        u32::try_from(height.max(1)).unwrap_or(1),
    )
}

/// A jacket that is plainly a picture, not a glyph and not a page of type.
///
/// Each book gets its own banding so two jackets on one shelf do not look
/// like the same file printed twice.
fn painted_cover(seed: usize, width: u32, height: u32) -> Vec<u8> {
    let width = usize::try_from(width).unwrap_or(0);
    let height = usize::try_from(height).unwrap_or(0);
    let mut grey = vec![0xE8_u8; width.saturating_mul(height)];
    if width == 0 || height == 0 {
        return grey;
    }
    let rule = (width / 48).max(2);
    let band = (height / 7).max(8);
    let shift = seed.saturating_mul(37);
    for y in 0..height {
        for x in 0..width {
            let edge = x < rule || y < rule || x >= width - rule || y >= height - rule;
            let header = y < band;
            let stripe = ((y + shift) / (band / 2).max(1)) % 2 == 0;
            let circle = {
                let cx = i32::try_from(width / 2).unwrap_or(0);
                let cy = i32::try_from(height / 2 + band / 2).unwrap_or(0);
                let dx = i32::try_from(x).unwrap_or(0) - cx;
                let dy = i32::try_from(y).unwrap_or(0) - cy;
                let radius = i32::try_from(width / 5).unwrap_or(0);
                dx.saturating_mul(dx) + dy.saturating_mul(dy) < radius.saturating_mul(radius)
            };
            grey[y * width + x] = if edge {
                0x20
            } else if header {
                0x38
            } else if circle {
                0x10
            } else if stripe {
                0xC8
            } else {
                0xA8
            };
        }
    }
    grey
}

fn main() -> ExitCode {
    match kobo_sdk::run("books", Books::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("books: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures, painted_cover, Books, Document, Kind, View, READING, SHELF_PAGE};
    use kobo_sdk::prelude::*;
    use kobo_sdk::{
        document_preview, DeviceResult, LibraryEntry, StoreRequest, StoreResult, CLARA_BW_METRICS,
    };
    use kobo_ui::{Chrome, LayoutKind, TileShape};

    fn shown(commands: Vec<Command>) -> Screen {
        commands
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("a screen")
    }

    fn words(screen: &Screen) -> String {
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        layout
            .nodes
            .iter()
            .flat_map(|node| &node.text_lines)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn open(runner: &mut AppRunner<Books>, kind: Kind) -> Screen {
        let index = runner
            .app()
            .documents
            .iter()
            .position(|document| document.kind == kind)
            .expect("a document of that kind");
        shown(runner.action(action_id(&format!("book-{index}"))))
    }

    /// A plain text book long enough to turn several pages.
    fn long_book() -> Document {
        let body = (1..=400)
            .map(|line| format!("Line {line} of a letter that runs on for page after page."))
            .collect::<Vec<_>>()
            .join("\n\n");
        Document {
            id: "0/long-letter.txt".to_owned(),
            title: "A Long Letter".to_owned(),
            kind: Kind::Text,
            preview: "Line 1".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some(body),
        }
    }

    /// Books started fresh, with its saved reading record handed back the way
    /// the store hands it back after a restart.
    fn started_with(record: Option<Vec<u8>>) -> AppRunner<Books> {
        let mut books = Books::empty();
        books.take_documents(vec![long_book()]);
        let mut runner = AppRunner::new(books);
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: READING.into(),
            value: record,
        });
        runner
    }

    fn saved(commands: &[Command]) -> Option<Vec<u8>> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == READING => {
                Some(value.clone())
            }
            _ => None,
        })
    }

    #[test]
    fn the_first_screen_fits_a_clara() {
        let mut runner = AppRunner::new(Books::seeded());
        let screen = shown(runner.start());
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
    }

    #[test]
    fn an_empty_library_says_so() {
        let mut runner = AppRunner::new(Books::empty());
        let screen = shown(runner.start());
        let text = words(&screen);
        assert!(
            text.contains("No books yet"),
            "an empty library did not say it was empty: {text}"
        );
        assert!(
            !screen.nodes.iter().any(|node| matches!(
                node,
                Node::TileGrid {
                    shape: TileShape::Portrait,
                    ..
                }
            )),
            "an empty library still drew a shelf"
        );
    }

    #[test]
    fn a_card_listing_draws_the_titles_the_runtime_named() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let screen = shown(runner.device_result(DeviceResult::Library {
            entries: vec![
                LibraryEntry {
                    id: "n/uuid-piranesi".to_owned(),
                    title: "Piranesi".to_owned(),
                    kind: 1,
                    bytes: 0,
                    on_card: false,
                },
                LibraryEntry {
                    id: "0/notes.md".to_owned(),
                    title: "Notes on a Rainy Afternoon".to_owned(),
                    kind: 2,
                    bytes: 80,
                    on_card: true,
                },
            ],
            truncated: false,
        }));
        let text = words(&screen);
        assert!(text.contains("Piranesi"), "{text}");
        assert!(text.contains("Notes on a Rainy Afternoon"), "{text}");
        assert!(
            !text.contains("The Open Sea"),
            "the fixture shelf came back instead of the card"
        );
    }

    #[test]
    fn a_store_title_that_is_not_on_the_card_says_so() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        runner.device_result(DeviceResult::Library {
            entries: vec![LibraryEntry {
                id: "n/uuid-piranesi".to_owned(),
                title: "Piranesi".to_owned(),
                kind: 1,
                bytes: 0,
                on_card: false,
            }],
            truncated: false,
        });
        let screen = shown(runner.action(action_id("book-0")));
        let text = words(&screen);
        assert!(
            text.contains("not downloaded yet") && text.contains("Kobo reader"),
            "opening an undownloaded title did not say why: {text}"
        );
        assert!(
            matches!(runner.app().view, View::Unreadable(_)),
            "an undownloaded title was opened as if the file were here"
        );
    }

    #[test]
    fn the_shelf_uses_portrait_tiles_with_pictures() {
        let mut runner = AppRunner::new(Books::seeded());
        let screen = shown(runner.start());
        let grid = screen.nodes.iter().find_map(|node| match node {
            Node::TileGrid {
                shape: TileShape::Portrait,
                tiles,
                ..
            } => Some(tiles),
            _ => None,
        });
        let tiles = grid.expect("a portrait shelf");
        assert_eq!(tiles.len(), fixtures().len().min(SHELF_PAGE));
        assert!(
            tiles.iter().all(|tile| tile.picture.is_some()),
            "a document was left on a generic glyph"
        );
        for (tile, document) in tiles.iter().zip(&runner.app().documents) {
            if document.opens_here() {
                assert!(
                    tile.subtitle.is_empty(),
                    "the format was written under {}",
                    document.title
                );
            } else {
                assert_eq!(tile.subtitle, "Opens in Kobo reader");
            }
        }
        let text = words(&screen);
        assert!(text.contains("The Open Sea"), "{text}");
        assert!(text.contains("Notes on a Rainy Afternoon"), "{text}");
        assert!(text.contains("Timetable"), "{text}");
    }

    #[test]
    fn a_coverless_tile_is_the_first_lines_not_a_glyph() {
        let notes = fixtures()
            .into_iter()
            .find(|document| !document.has_cover && document.kind == Kind::Markdown)
            .expect("a coverless markdown document");
        let grey = document_preview(&notes.preview, notes.kind.badge(), 180, 280);
        assert!(
            grey.iter().any(|pixel| *pixel < 40),
            "the first lines left no ink"
        );
        assert!(
            notes.preview.contains("The rain started"),
            "the preview was not the document"
        );
        assert_eq!(notes.kind.badge(), "md");
    }

    #[test]
    fn a_jacket_is_a_picture_and_not_a_page_of_type() {
        let cover = painted_cover(0, 180, 280);
        let preview = document_preview(
            "The tide was already turning when the boat left the harbour.",
            "epub",
            180,
            280,
        );
        assert_ne!(
            cover, preview,
            "the jacket was just the first-lines page again"
        );
        assert!(
            cover.iter().filter(|pixel| **pixel < 40).count() > 80,
            "the jacket had no dark artwork"
        );
    }

    #[test]
    fn an_epub_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Epub);
        assert_eq!(runner.app().view, View::Reading);
        assert!(
            runner.app().book.is_open(),
            "the EPUB tap did not hold a reader"
        );
        let text = words(&screen);
        assert!(
            text.contains("The tide was already turning"),
            "the shared reader did not show the EPUB: {text}"
        );
        assert!(
            !screen
                .nodes
                .iter()
                .any(|node| matches!(node, Node::TileGrid { .. })),
            "the EPUB tap left the shelf on the panel"
        );
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
    }

    #[test]
    fn markdown_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Markdown);
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().book.is_open());
        let text = words(&screen);
        assert!(
            text.contains("The rain started"),
            "the shared reader did not show the Markdown: {text}"
        );
    }

    #[test]
    fn plain_text_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Text);
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().book.is_open());
        let text = words(&screen);
        assert!(
            text.contains("Dear M"),
            "the shared reader did not show the text file: {text}"
        );
    }

    #[test]
    fn html_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Html);
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().book.is_open());
        let text = words(&screen);
        assert!(
            text.contains("Market Street"),
            "the shared reader did not show the HTML: {text}"
        );
    }

    #[test]
    fn a_pdf_is_listed_and_will_not_open() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Pdf);
        let text = words(&screen);
        assert!(
            text.contains("PDFs open in the Kobo reader"),
            "opening a PDF did not say where to read it: {text}"
        );
        assert!(
            matches!(runner.app().view, View::Unreadable(_)),
            "a PDF tap left the shelf as if nothing happened"
        );
        assert!(
            !runner.app().book.is_open(),
            "a PDF was opened in the reader"
        );
        runner.action(ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Shelf,
            "the bar's back arrow did not return to the shelf"
        );
    }

    #[test]
    fn the_shelf_is_tappable_on_a_clara() {
        let mut runner = AppRunner::new(Books::seeded());
        let screen = shown(runner.start());
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let controls: Vec<_> = layout
            .nodes
            .iter()
            .filter(|node| node.kind.acts_on().is_some())
            .collect();
        assert!(
            !controls.is_empty(),
            "the shelf offered nothing a finger could reach"
        );
        for control in &controls {
            assert!(
                control.rect.width >= CLARA_BW_METRICS.touch_target_minimum()
                    && control.rect.height >= CLARA_BW_METRICS.touch_target_minimum(),
                "{:?} is smaller than a touch target",
                control.kind
            );
        }
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::FramedPicture(_))),
            "the shelf drew glyphs where the covers should be"
        );
    }

    #[test]
    fn a_book_reopens_at_the_page_and_text_size_it_was_closed_at() {
        let mut runner = started_with(None);
        runner.action(action_id("book-0"));
        assert_eq!(runner.app().view, View::Reading);
        let mut record = None;
        for _ in 0..3 {
            record = saved(&runner.page_turn(true)).or(record);
            runner.store_result(StoreResult::Saved {
                key: READING.into(),
            });
        }
        record = saved(&runner.action(action_id(kobo_read::action::LARGER))).or(record);
        runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        let left = runner.app().book.memory().cloned().expect("a book is open");
        assert!(left.at > 0, "three page turns left the book at its start");
        assert_ne!(left.scale, kobo_read::Memory::default().scale);
        record = saved(&runner.action(ActionId::BACK)).or(record);
        assert_eq!(runner.app().view, View::Shelf);
        let record = record.expect("closing a book wrote its place down");

        // Books restarts with only what the store kept.
        let mut runner = started_with(Some(record));
        runner.action(action_id("book-0"));
        let reopened = runner
            .app()
            .book
            .memory()
            .cloned()
            .expect("the book opened again");
        assert_eq!(reopened.at, left.at, "the book reopened at another page");
        assert_eq!(
            reopened.scale, left.scale,
            "the book reopened at another size"
        );
    }

    #[test]
    fn quick_page_turns_and_leaving_still_keep_the_last_page() {
        let mut runner = started_with(None);
        runner.action(action_id("book-0"));
        runner.page_turn(true);
        runner.page_turn(true);
        runner.lifecycle(Lifecycle::Background);
        let left = runner.app().book.memory().map(|memory| memory.at);
        // The second turn waited for the first write; its answer releases it.
        let commands = runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        let record =
            saved(&commands).expect("the last page was written once the first write was kept");
        let mut runner = started_with(Some(record));
        runner.action(action_id("book-0"));
        assert_eq!(runner.app().book.memory().map(|memory| memory.at), left);
    }

    #[test]
    fn a_place_that_was_not_saved_offers_retry_saving_on_the_shelf_and_writes_it_again() {
        let mut runner = started_with(None);
        runner.action(action_id("book-0"));
        runner.page_turn(true);
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Unwritable));
        // Leaving the book tries once more; when that fails too, the shelf
        // is where the owner learns of it.
        runner.action(ActionId::BACK);
        let shelf =
            shown(runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Unwritable)));
        let drawn = words(&shelf);
        assert!(drawn.contains("could not save your place"), "{drawn}");
        assert!(drawn.contains("Retry saving"), "{drawn}");
        // The fix is on the banner that explains it, and the shelf still fits.
        let layout = shelf.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout
            .nodes
            .iter()
            .any(|node| { node.kind == LayoutKind::BannerAction(action_id("retry-save")) }));
        assert!(shelf.validate(&CLARA_BW_METRICS).is_empty());
        let commands = runner.action(action_id("retry-save"));
        assert!(
            saved(&commands).is_some(),
            "Retry saving writes the place now"
        );
        let commands = runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        assert!(!words(&shown(commands)).contains("Retry saving"));
    }

    #[test]
    fn books_that_open_here_come_before_those_that_open_in_the_kobo_reader() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        runner.device_result(DeviceResult::Library {
            entries: vec![
                LibraryEntry {
                    id: "0/timetable.pdf".to_owned(),
                    title: "Timetable".to_owned(),
                    kind: 5,
                    bytes: 80,
                    on_card: true,
                },
                LibraryEntry {
                    id: "n/uuid-piranesi".to_owned(),
                    title: "Piranesi".to_owned(),
                    kind: 1,
                    bytes: 0,
                    on_card: false,
                },
                LibraryEntry {
                    id: "0/notes.md".to_owned(),
                    title: "Notes".to_owned(),
                    kind: 2,
                    bytes: 80,
                    on_card: true,
                },
            ],
            truncated: false,
        });
        let order = runner
            .app()
            .documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, ["Notes", "Timetable", "Piranesi"]);
    }

    #[test]
    fn an_empty_library_says_how_to_add_books() {
        let mut runner = AppRunner::new(Books::empty());
        let text = words(&shown(runner.start()));
        assert!(text.contains("copy EPUB"), "{text}");
    }

    #[test]
    fn a_library_that_could_not_be_listed_offers_to_try_again() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let screen = shown(runner.device_result(DeviceResult::Failed(DeviceError::Backend)));
        let text = words(&screen);
        assert!(text.contains("could not be read"), "{text}");
        assert!(!text.contains("No books yet"), "{text}");
        let commands = runner.action(action_id("relist"));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ListLibrary))));
    }
}
