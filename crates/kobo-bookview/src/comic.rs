//! Shared comic reading controls. Apps own their shelves, imports and durable
//! save acknowledgements; this view owns page navigation and presentation.

use kobo_comic::{reader::Reader, viewport::Fit, ComicError};
use kobo_sdk::{
    action_id, ActionId, Context, DisplayMetrics, Glyph, PictureHandle, Screen, ScreenBuilder,
    TilePicture, TileShape,
};

const PAGE: PictureHandle = PictureHandle(2000);
const THUMB_BASE: u32 = 2001;
const THUMBS: usize = 2;

#[cfg(test)]
#[path = "comic_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    #[default]
    Page,
    Controls,
    Options,
    Details,
    Pan,
    Pages,
    Jump,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Changed,
    RetrySave,
    Exit,
    Elsewhere,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SaveState {
    #[default]
    Saved,
    Pending,
    Failed,
}

#[derive(Debug)]
pub struct ComicView {
    reader: Reader,
    title: String,
    metrics: DisplayMetrics,
    mode: Mode,
    picture: Option<TilePicture>,
    previews: Vec<Option<TilePicture>>,
    preview_start: usize,
    number: String,
    orientation: kobo_ui::Orientation,
    notice: Option<String>,
    save_state: SaveState,
}

impl ComicView {
    /// Opens without taking ownership of persistence or claiming a save.
    ///
    /// # Errors
    /// Returns the bounded archive admission error.
    pub fn open(context: &mut Context, bytes: Vec<u8>, title: &str) -> Result<Self, ComicError> {
        let reader = Reader::open(bytes)?;
        let title = reader
            .comic()
            .metadata
            .title
            .clone()
            .unwrap_or_else(|| title.to_owned());
        let notice = reader.comic().metadata.warning.clone();
        let mut view = Self {
            reader,
            title,
            metrics: context.metrics(),
            mode: Mode::Page,
            picture: None,
            previews: Vec::new(),
            preview_start: 0,
            number: String::new(),
            orientation: kobo_ui::Orientation::Portrait,
            notice,
            save_state: SaveState::Saved,
        };
        view.paint(context);
        Ok(view)
    }
    #[must_use]
    pub const fn reader(&self) -> &Reader {
        &self.reader
    }

    /// Returns bytes for an acknowledged store write owned by the app.
    ///
    /// # Errors
    /// Refuses invalid or excessive reading memory.
    pub fn memory(&self) -> Result<Vec<u8>, String> {
        self.reader.memory().encode(self.reader.comic())
    }

    pub fn set_save_state(&mut self, state: SaveState) {
        self.save_state = state;
    }

    /// Restores a position, retaining the current page and showing guidance if
    /// its record is corrupt or from a newer app.
    pub fn restore(&mut self, context: &mut Context, bytes: Option<&[u8]>) {
        if let Err(error) = self.reader.restore(bytes) {
            self.notice = Some(error);
        }
        self.paint(context);
    }
    pub fn set_direction(&mut self, context: &mut Context, rtl: bool) {
        self.reader.memory_mut().right_to_left = rtl;
        self.paint(context);
    }
    pub fn reflow(&mut self, context: &mut Context, metrics: DisplayMetrics) {
        self.metrics = metrics;
        self.paint(context);
    }
    pub fn close(&mut self, context: &mut Context) {
        context.set_orientation(kobo_ui::Orientation::Portrait);
        context.drop_picture(PAGE);
        for index in 0..THUMBS {
            context.drop_picture(PictureHandle(
                THUMB_BASE + u32::try_from(index).unwrap_or(0),
            ));
        }
        self.picture = None;
        self.previews.clear();
    }

    fn page_header(&self) -> ScreenBuilder {
        let visible = self.reader.visible_pages(self.target_without_header());
        let position = if visible.len() == 2 {
            format!(
                "Pages {}–{} of {}",
                visible[0] + 1,
                visible[1] + 1,
                self.reader.comic().pages.len()
            )
        } else {
            format!(
                "Page {} of {}",
                self.reader.memory().page + 1,
                self.reader.comic().pages.len()
            )
        };
        let position = match self.save_state {
            SaveState::Saved => position,
            SaveState::Pending => format!("{position} · Saving"),
            SaveState::Failed => format!("{position} · Position not saved"),
        };
        ScreenBuilder::new("comic-page")
            .top_bar(&self.title)
            .top_bar_action("comic-controls", "Reading")
            .reading(true)
            .owns_back(true)
            .secondary(position)
    }
    fn target_without_header(&self) -> (u32, u32) {
        (
            u32::try_from(self.metrics.width.max(1)).unwrap_or(1),
            u32::try_from(self.metrics.height.max(1)).unwrap_or(1),
        )
    }
    fn target(&self) -> (u32, u32) {
        let mut header = self.page_header();
        if self.mode == Mode::Pan {
            header = header
                .buttons([("comic-left", "Left"), ("comic-right", "Right")])
                .buttons([("comic-up", "Up"), ("comic-down", "Down")])
                .bottom_action("comic-read", "Read");
        }
        let screen = header.build();
        let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
        let layout = screen.layout_with(&self.metrics, &chrome);
        (
            u32::try_from(layout.content.width.max(1)).unwrap_or(1),
            u32::try_from(
                (layout.content.height
                    - layout.content_used()
                    - 3 * self.metrics.space(kobo_ui::Space::Small))
                .max(1),
            )
            .unwrap_or(1),
        )
    }

    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "one explicit map of reader modes to screens"
    )]
    pub fn screen(&self) -> Screen {
        match self.mode {
            Mode::Page | Mode::Pan => {
                let mut screen = self.page_header();
                if let Some(picture) = self.picture {
                    let max_mm = u16::try_from(
                        u64::from(self.target().1) * 254
                            / (u64::try_from(self.metrics.pixels_per_inch)
                                .unwrap_or(300)
                                .max(1)
                                * 10),
                    )
                    .unwrap_or(u16::MAX);
                    screen = screen.unframed_picture(picture, max_mm);
                } else {
                    screen = screen.text(self.notice.as_deref().unwrap_or(
                        "This page could not be shown. Use Reading to choose another page.",
                    ));
                }
                if self.mode == Mode::Pan {
                    return screen
                        .buttons([("comic-left", "Left"), ("comic-right", "Right")])
                        .buttons([("comic-up", "Up"), ("comic-down", "Down")])
                        .bottom_action("comic-read", "Read")
                        .build();
                }
                let (previous, next) = if self.reader.memory().right_to_left {
                    ("comic-next", "comic-previous")
                } else {
                    ("comic-previous", "comic-next")
                };
                screen.page_turns(previous, next).build()
            }
            Mode::Controls => {
                let memory = self.reader.memory();
                let mut screen = ScreenBuilder::new("comic-controls")
                    .top_bar("Reading")
                    .owns_back(true)
                    .secondary(format!(
                        "{} · {}%",
                        if memory.viewport.fit == Fit::Page {
                            "Fit page"
                        } else {
                            "Fit width"
                        },
                        memory.viewport.zoom()
                    ));
                if self.notice.is_some() {
                    screen = screen.top_bar_action("comic-details", "Details");
                }
                screen
                    .buttons([
                        ("comic-fit-page", "Fit page"),
                        ("comic-fit-width", "Fit width"),
                    ])
                    .buttons([("comic-zoom-out", "Zoom out"), ("comic-zoom-in", "Zoom in")])
                    .buttons([("comic-pan", "Move page"), ("comic-options", "More")])
                    .buttons([("comic-pages", "Pages"), ("comic-jump", "Go to page")])
                    .bottom_action(
                        if self.save_state == SaveState::Failed {
                            "comic-save-retry"
                        } else {
                            "comic-read"
                        },
                        if self.save_state == SaveState::Failed {
                            "Retry saving"
                        } else {
                            "Read"
                        },
                    )
                    .build()
            }
            Mode::Details => ScreenBuilder::new("comic-details")
                .top_bar("Comic details")
                .owns_back(true)
                .text(self.notice.as_deref().unwrap_or("No further details."))
                .bottom_action("comic-read", "Read")
                .build(),
            Mode::Options => ScreenBuilder::new("comic-options")
                .top_bar("Reading options")
                .owns_back(true)
                .button(
                    "comic-direction",
                    if self.reader.memory().right_to_left {
                        "Right to left"
                    } else {
                        "Left to right"
                    },
                )
                .button(
                    "comic-spreads",
                    if self.reader.memory().spreads {
                        "Two-page spreads"
                    } else {
                        "Single pages"
                    },
                )
                .button("comic-rotate", "Rotate page")
                .secondary("Spreads appear in landscape. Covers stay on their own.")
                .bottom_action("comic-read", "Read")
                .build(),
            Mode::Pages => ScreenBuilder::new("comic-pages")
                .top_bar("Pages")
                .owns_back(true)
                .picture_tiles(
                    TileShape::Square,
                    (0..self.previews.len()).map(|index| {
                        (
                            format!("comic-page-{index}"),
                            format!("Page {}", self.preview_start + index + 1),
                            Glyph::Book,
                            self.previews[index],
                        )
                    }),
                )
                .buttons([
                    ("comic-thumbs-previous", "Previous"),
                    ("comic-thumbs-next", "Next"),
                ])
                .bottom_action("comic-read", "Return to reading")
                .build(),
            Mode::Jump => ScreenBuilder::new("comic-jump")
                .top_bar("Go to page")
                .owns_back(true)
                .secondary(format!(
                    "Enter a page from 1 to {}",
                    self.reader.comic().pages.len()
                ))
                .heading(if self.number.is_empty() {
                    "Page number".into()
                } else {
                    self.number.clone()
                })
                .grid(
                    if self.metrics.width > self.metrics.height {
                        6
                    } else {
                        3
                    },
                    false,
                    (1..=9)
                        .map(|n| (format!("comic-digit-{n}"), n.to_string()))
                        .chain([
                            ("comic-delete".into(), "Delete".into()),
                            ("comic-digit-0".into(), "0".into()),
                            ("comic-go".into(), "Go".into()),
                        ]),
                )
                .build(),
        }
    }

    fn paint(&mut self, context: &mut Context) {
        let target = self.target();
        match self.reader.render(target) {
            Ok(picture) => {
                self.picture = context.put_picture(
                    PAGE,
                    picture.width(),
                    picture.height(),
                    picture.into_grey(),
                );
                if self.picture.is_none() {
                    self.notice = Some(
                        "There is not enough picture memory. Close and reopen this comic.".into(),
                    );
                }
            }
            Err(error) => {
                self.picture = None;
                self.notice = Some(error.to_string());
            }
        }
    }
    fn preview(&mut self, context: &mut Context) {
        self.previews.clear();
        for index in
            self.preview_start..(self.preview_start + THUMBS).min(self.reader.comic().pages.len())
        {
            let handle =
                PictureHandle(THUMB_BASE + u32::try_from(index - self.preview_start).unwrap_or(0));
            context.drop_picture(handle);
            let preview =
                self.reader.thumbnail(index).ok().and_then(|p| {
                    context.put_picture(handle, p.width(), p.height(), p.into_grey())
                });
            self.previews.push(preview);
        }
    }

    pub fn turn(&mut self, context: &mut Context, forward: bool) -> bool {
        let changed = self.reader.turn(forward, self.target());
        if changed {
            self.notice = None;
            self.paint(context);
        }
        changed
    }

    fn jump_action(&mut self, context: &mut Context, action: ActionId) -> Outcome {
        if let Some(digit) = (0..=9).find(|n| action == action_id(&format!("comic-digit-{n}"))) {
            if self.number.len() < 4 {
                self.number.push(char::from(b'0' + digit));
            }
        } else if action == action_id("comic-delete") {
            self.number.pop();
        } else if action == action_id("comic-go") {
            if let Some(page) = self
                .number
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .filter(|&n| n < self.reader.comic().pages.len())
            {
                self.reader.jump(page);
                self.mode = Mode::Page;
                self.notice = None;
                self.paint(context);
            } else {
                self.notice = Some(format!(
                    "Choose a page from 1 to {}.",
                    self.reader.comic().pages.len()
                ));
                self.mode = Mode::Controls;
            }
        } else {
            return Outcome::Elsewhere;
        }
        Outcome::Changed
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive reader action dispatcher"
    )]
    pub fn act(&mut self, context: &mut Context, action: ActionId) -> Outcome {
        if action == action_id("comic-save-retry") && self.save_state == SaveState::Failed {
            return Outcome::RetrySave;
        }
        if action == ActionId::BACK {
            if self.mode == Mode::Page {
                return Outcome::Exit;
            }
            self.mode = Mode::Page;
            self.paint(context);
            return Outcome::Changed;
        }
        if self.mode == Mode::Jump {
            return self.jump_action(context, action);
        }
        for index in 0..self.previews.len() {
            if self.mode == Mode::Pages && action == action_id(&format!("comic-page-{index}")) {
                self.reader.jump(self.preview_start + index);
                self.mode = Mode::Page;
                self.notice = None;
                self.paint(context);
                return Outcome::Changed;
            }
        }
        let name = [
            "comic-controls",
            "comic-options",
            "comic-details",
            "comic-pan",
            "comic-rotate",
            "comic-read",
            "comic-pages",
            "comic-jump",
            "comic-fit-page",
            "comic-fit-width",
            "comic-zoom-in",
            "comic-zoom-out",
            "comic-left",
            "comic-right",
            "comic-up",
            "comic-down",
            "comic-direction",
            "comic-spreads",
            "comic-next",
            "comic-previous",
            "comic-thumbs-next",
            "comic-thumbs-previous",
        ]
        .into_iter()
        .find(|name| action_id(name) == action);
        let Some(name) = name else {
            return Outcome::Elsewhere;
        };
        match name {
            "comic-controls" => self.mode = Mode::Controls,
            "comic-options" => self.mode = Mode::Options,
            "comic-details" => self.mode = Mode::Details,
            "comic-read" => self.mode = Mode::Page,
            "comic-pan" => self.mode = Mode::Pan,
            "comic-rotate" => {
                self.orientation = if self.orientation == kobo_ui::Orientation::Portrait {
                    kobo_ui::Orientation::Landscape
                } else {
                    kobo_ui::Orientation::Portrait
                };
                self.metrics = context.metrics().oriented(self.orientation);
                context.set_orientation(self.orientation);
                self.mode = Mode::Page;
            }
            "comic-pages" => {
                self.mode = Mode::Pages;
                self.preview_start = self.reader.memory().page / THUMBS * THUMBS;
                self.preview(context);
            }
            "comic-jump" => {
                self.mode = Mode::Jump;
                self.number.clear();
            }
            "comic-fit-page" => self.reader.memory_mut().viewport.reset(Fit::Page),
            "comic-fit-width" => self.reader.memory_mut().viewport.reset(Fit::Width),
            "comic-zoom-in" => self.reader.memory_mut().viewport.zoom_by(25),
            "comic-zoom-out" => self.reader.memory_mut().viewport.zoom_by(-25),
            "comic-left" => {
                self.reader.memory_mut().viewport.pan(-2500, 0);
            }
            "comic-right" => {
                self.reader.memory_mut().viewport.pan(2500, 0);
            }
            "comic-up" => {
                self.reader.memory_mut().viewport.pan(0, -2500);
            }
            "comic-down" => {
                self.reader.memory_mut().viewport.pan(0, 2500);
            }
            "comic-direction" => {
                self.reader.memory_mut().right_to_left = !self.reader.memory().right_to_left;
            }
            "comic-spreads" => self.reader.memory_mut().spreads = !self.reader.memory().spreads,
            "comic-next" => {
                self.turn(context, true);
                return Outcome::Changed;
            }
            "comic-previous" => {
                self.turn(context, false);
                return Outcome::Changed;
            }
            "comic-thumbs-next"
                if self.preview_start + THUMBS < self.reader.comic().pages.len() =>
            {
                self.preview_start += THUMBS;
                self.preview(context);
            }
            "comic-thumbs-previous" => {
                self.preview_start = self.preview_start.saturating_sub(THUMBS);
                self.preview(context);
            }
            _ => {}
        }
        if matches!(self.mode, Mode::Page | Mode::Controls | Mode::Pan) {
            self.paint(context);
        }
        Outcome::Changed
    }
}
