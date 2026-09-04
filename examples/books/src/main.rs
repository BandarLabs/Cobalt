//! The bundled library: documents already on this device.
//!
//! Books is an included application. It lists files on the card (`/mnt/onboard`,
//! `/mnt/sd`, Cobalt shelves) and titles from the stock Kobo library. EPUB,
//! Markdown, HTML and plain text open in the shared reader. PDF is listed
//! and not opened. A Kobo Store title that has not been downloaded is listed
//! and says so when opened.

use kobo_bookview::{BookView, Step};
use kobo_read::{Memory, Outcome};
use kobo_sdk::{
    action_id, document_preview, stamp_format_badge, ActionId, Context, DeviceRequest, DeviceResult,
    Glyph, KoboApp, LibraryEntry, PictureHandle, Screen, ScreenBuilder, TaskId, TaskOutcome, Tile,
    TilePicture, TileShape,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Epub,
    Markdown,
    Html,
    Text,
    Pdf,
}

impl Kind {
    const fn badge(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Markdown => "md",
            Self::Html => "html",
            Self::Text => "txt",
            Self::Pdf => "pdf",
        }
    }

    const fn is_readable(self) -> bool {
        !matches!(self, Self::Pdf)
    }

    const fn filename(self) -> &'static str {
        match self {
            Self::Epub => "open-sea.epub",
            Self::Markdown => "notes.md",
            Self::Html => "streets.html",
            Self::Text => "letter.txt",
            Self::Pdf => "timetable.pdf",
        }
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
            body: Some(
                "The tide was already turning when the boat left the harbour.".to_owned(),
            ),
        },
        Document {
            id: "test/notes.md".to_owned(),
            title: "Notes on a Rainy Afternoon".to_owned(),
            kind: Kind::Markdown,
            preview: "The rain started before the kettle boiled.".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some("# Notes on a Rainy Afternoon\n\nThe rain started before the kettle boiled.".to_owned()),
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
            body: Some("Dear M,\n\nThe hill behind the house is still green in September.".to_owned()),
        },
        Document {
            id: "test/streets.html".to_owned(),
            title: "Index of Streets".to_owned(),
            kind: Kind::Html,
            preview: "Market Street runs east from the harbour.".to_owned(),
            on_card: true,
            has_cover: false,
            body: Some("<h1>Index of Streets</h1>\n<p>Market Street runs east from the harbour.</p>".to_owned()),
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
}

impl Default for Books {
    fn default() -> Self {
        Self::empty()
    }
}

impl Books {
    #[cfg(test)]
    fn seeded() -> Self {
        Self {
            view: View::Shelf,
            page: 0,
            documents: fixtures(),
            book: BookView::new(),
            open_title: None,
        }
    }

    fn empty() -> Self {
        Self {
            view: View::Shelf,
            page: 0,
            documents: Vec::new(),
            book: BookView::new(),
            open_title: None,
        }
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Shelf => self.shelf(context),
            View::Opening(_) => self.shelf(context),
            View::Reading => self.reading(),
            View::Unreadable(index) => self.unreadable(index),
        };
        context.set_screen(screen);
    }

    fn shelf(&mut self, context: &mut Context) -> Screen {
        let mut screen = ScreenBuilder::new("books").top_bar("Books");
        if self.documents.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Book),
                    "No books yet",
                    "Documents already on this device will appear here.",
                )
                .build();
        }
        let pages = self.pages(context);
        self.page = self.page.min(pages.len().saturating_sub(1));
        let page = pages.get(self.page).cloned().unwrap_or_default();
        let (width, height) = tile_pixels(context);
        let tiles = page.into_iter().filter_map(|index| {
            let document = self.documents.get(index)?;
            let title = document.title.clone();
            let picture = self.picture(context, index, width, height);
            Some((
                format!("book-{index}"),
                title,
                Glyph::Book,
                move |tile: Tile| match picture {
                    Some(picture) => tile.with_picture(picture),
                    None => tile,
                },
            ))
        });
        screen = screen.tile_grid(TileShape::Portrait, tiles);
        let page_count = pages.len().max(1);
        if page_count > 1 {
            let page = u16::try_from(self.page + 1).unwrap_or(u16::MAX);
            let total = u16::try_from(page_count).unwrap_or(u16::MAX);
            screen = screen
                .page_turns(PREVIOUS, NEXT)
                .page_position(page, total);
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
                "This document is listed but is not openable yet.",
            )
            .button(BACK, "Back")
            .build()
    }

    fn unreadable(&self, index: usize) -> Screen {
        let document = self.documents.get(index);
        let title = document.map_or("This document", |document| document.title.as_str());
        let reason = if document.is_some_and(|document| !document.on_card) {
            "This book is in the Kobo library but is not on the card. Open it in the Kobo reader to download."
        } else if document.is_some_and(|document| !document.kind.is_readable()) {
            "PDF is listed but is not readable yet."
        } else {
            "This document is listed but is not openable yet."
        };
        ScreenBuilder::new("books-unreadable")
            .top_bar("Books")
            .splash(Some(Glyph::Book), title, reason)
            .button(BACK, "Back")
            .build()
    }

    fn pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let pages = context.paginate_tiles(self.documents.len(), TileShape::Portrait, false);
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
        if !document.on_card || !document.kind.is_readable() {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
        }
        if let Some(body) = document.body.as_ref() {
            let bytes = fixture_bytes(&document, body);
            self.finish_open(context, index, &document, bytes);
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
        bytes: Vec<u8>,
    ) {
        if bytes.is_empty() {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
        }
        match self.book.open_bytes(
            context,
            document.kind.filename(),
            &bytes,
            Memory::default(),
        ) {
            Ok(()) => {
                self.open_title = Some(document.title.clone());
                self.view = View::Reading;
            }
            Err(_) => {
                self.book.close(context);
                self.open_title = None;
                self.view = View::Unreadable(index);
            }
        }
        self.show(context);
    }

    fn close_reader(&mut self, context: &mut Context) {
        self.book.close(context);
        self.open_title = None;
        self.view = View::Shelf;
        self.show(context);
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
                    self.show(context);
                }
                Outcome::Elsewhere | Outcome::Repaint | Outcome::Save => self.show(context),
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
    fn on_start(&mut self, context: &mut Context) {
        if self.documents.is_empty() {
            context.device().list_library();
        }
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.read_action(context, action) {
            return;
        }
        if action == action_id(BACK) {
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
        if let Some(index) = (0..self.documents.len())
            .find(|index| action == action_id(&format!("book-{index}")))
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
                    self.documents = entries.into_iter().filter_map(from_entry).collect();
                    self.view = View::Shelf;
                    self.show(context);
                }
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                    self.documents.clear();
                    self.view = View::Shelf;
                    self.show(context);
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
                            self.finish_open(context, index, &document, bytes);
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

fn from_entry(entry: LibraryEntry) -> Option<Document> {
    let kind = match entry.kind {
        1 => Kind::Epub,
        2 => Kind::Markdown,
        3 => Kind::Html,
        4 => Kind::Text,
        5 => Kind::Pdf,
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
        Kind::Pdf => Vec::new(),
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
    use super::{fixtures, painted_cover, Books, Kind, View, SHELF_PAGE};
    use kobo_sdk::prelude::*;
    use kobo_sdk::{document_preview, DeviceResult, LibraryEntry, CLARA_BW_METRICS};
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
        let index = fixtures()
            .iter()
            .position(|document| document.kind == kind)
            .expect("a document of that kind");
        shown(runner.action(action_id(&format!("book-{index}"))))
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
            text.contains("not on the card"),
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
        assert!(
            tiles.iter().all(|tile| tile.subtitle.is_empty()),
            "the format was written under the title"
        );
        let text = words(&screen);
        assert!(text.contains("The Open Sea"), "{text}");
        assert!(text.contains("Notes on a Rainy Afternoon"), "{text}");
        assert!(text.contains("Timetable"), "{text}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::TileSubtitle),
            "a format caption was drawn under a title"
        );
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
            !screen.nodes.iter().any(|node| matches!(node, Node::TileGrid { .. })),
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
            text.contains("not readable yet"),
            "opening a PDF did not say it was unreadable: {text}"
        );
        assert!(
            matches!(runner.app().view, View::Unreadable(_)),
            "a PDF tap left the shelf as if nothing happened"
        );
        assert!(
            !runner.app().book.is_open(),
            "a PDF was opened in the reader"
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
}
