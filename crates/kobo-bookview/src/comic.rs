//! Shared comic reading controls. Apps own their shelves, imports and durable
//! save acknowledgements; this view owns page navigation and presentation.

use kobo_comic::{reader::Reader, viewport::Fit, ComicError};
use kobo_sdk::{
    action_id, ActionId, Context, DisplayMetrics, Glyph, PictureHandle, Screen, ScreenBuilder,
    TilePicture, TileShape,
};

const PAGE: PictureHandle = PictureHandle(2000);
const THUMB_BASE: u32 = 2001;
/// The zoom the viewport allows, in percent, so the stepper can say which
/// end has nowhere further to go.
const MIN_ZOOM: u16 = 100;
const MAX_ZOOM: u16 = 400;

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
    /// Why the page number typed was refused, said on the keypad it was
    /// typed on until the next key.
    jump_refused: Option<String>,
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
            jump_refused: None,
            save_state: SaveState::Saved,
        };
        view.paint(context);
        Ok(view)
    }
    #[must_use]
    pub const fn reader(&self) -> &Reader {
        &self.reader
    }

    /// A bounded cover preview for the owning app's shelf, without moving its page.
    ///
    /// # Errors
    /// Returns the cover page's archive or image decode error.
    pub fn cover_preview(&mut self) -> Result<kobo_image::Picture, ComicError> {
        self.reader
            .thumbnail(self.reader.comic().metadata.cover.unwrap_or(0))
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
    /// Call with the result of the host identity query, not an inferred model.
    pub fn set_colour(&mut self, context: &mut Context, supported: bool) {
        self.reader.set_colour(supported);
        self.paint(context);
        if self.mode == Mode::Pages {
            self.preview(context);
        }
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
        for index in 0..4 {
            context.drop_picture(PictureHandle(
                THUMB_BASE + u32::try_from(index).unwrap_or(0),
            ));
        }
        self.picture = None;
        self.previews.clear();
    }

    /// Where the reader is, and whether that has been kept.
    fn position(&self) -> String {
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
        match self.save_state {
            SaveState::Saved => position,
            SaveState::Pending => format!("{position} · Saving"),
            SaveState::Failed => format!("{position} · Position not saved"),
        }
    }

    fn page_header(&self) -> ScreenBuilder {
        let screen = ScreenBuilder::new("comic-page")
            .top_bar(&self.title)
            .top_bar_action("comic-controls", "Reading")
            .reading(true)
            .owns_back(true);
        // On a page that has the panel the position moves to the controls,
        // a tap in the middle away: a caption above the art pushed it down
        // the panel. A position that could not be saved is still said here,
        // because that is something the reader needs to know unasked.
        if self.full_page() && self.save_state != SaveState::Failed {
            screen
        } else {
            screen.secondary(self.position())
        }
    }
    fn target_without_header(&self) -> (u32, u32) {
        (
            u32::try_from(self.metrics.width.max(1)).unwrap_or(1),
            u32::try_from(self.metrics.height.max(1)).unwrap_or(1),
        )
    }
    /// Whether the reader has asked for the page to have the panel to itself.
    ///
    /// Kept with the comic's own reading position rather than shared with the
    /// prose reader: the two keep separate records, and a page of art and a
    /// page of prose are not obliged to want the same thing.
    fn full_page(&self) -> bool {
        self.reader.memory().full_page
    }

    fn target(&self) -> (u32, u32) {
        // Decoded for the panel, not for the panel less a bar. A page fitted
        // under a header and then drawn without one is not bigger, it is the
        // same page with the header's height as white space beneath it: the
        // layout never scales a picture up to fill the room it is given.
        // Moving the page puts its controls under it whatever the page was
        // asked for, so they are measured in; a full page sized for the
        // whole panel pushed them off it.
        if self.full_page() && self.mode != Mode::Pan {
            return self.target_without_header();
        }
        let mut header = self.page_header();
        if self.mode == Mode::Pan {
            header = header
                .buttons([("comic-left", "Left"), ("comic-right", "Right")])
                .buttons([("comic-up", "Up"), ("comic-down", "Down")])
                .bottom_action("comic-read", "Read");
        }
        let screen = header.build();
        let layout = screen.layout_with(&self.metrics, &measured_chrome(&screen));
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

    /// The reading controls, with the name of each group when `named`, and
    /// the three groups side by side when `columns`.
    fn controls(&self, named: bool, columns: bool) -> Screen {
        let memory = self.reader.memory();
        let zoom = memory.viewport.zoom();
        let fit = usize::from(memory.viewport.fit != Fit::Page);
        let section = move |screen: ScreenBuilder, name: &str| {
            if named {
                screen.section(name)
            } else {
                screen
            }
        };
        let page_size = move |screen: ScreenBuilder| {
            section(screen, "Page size")
                .segmented(
                    fit,
                    [
                        ("comic-fit-page", "Fit page"),
                        ("comic-fit-width", "Fit width"),
                    ],
                )
                .stepper(
                    format!("Zoom {zoom}%"),
                    "comic-zoom-out",
                    Glyph::Minus,
                    "comic-zoom-in",
                    Glyph::Plus,
                )
                .stepper_ends(zoom > MIN_ZOOM, zoom < MAX_ZOOM)
        };
        let mut screen = ScreenBuilder::new("comic-controls")
            .top_bar("Reading")
            .owns_back(true)
            .secondary(self.position());
        if self.notice.is_some() {
            screen = screen.top_bar_action("comic-details", "Details");
        }
        let screen = if columns {
            // Held sideways the panel is too short for the groups one under
            // another, and Move page and Reading options, the way to turn
            // the page back, fell off its foot. Side by side they all fit.
            type Slot = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>;
            let slots: [(kobo_sdk::SlotWidth, Slot); 3] = [
                (kobo_sdk::SlotWidth::Fill, Box::new(page_size)),
                (
                    kobo_sdk::SlotWidth::Fill,
                    Box::new(move |slot| {
                        section(slot, "Pages")
                            .button("comic-pages", "All pages")
                            .button("comic-jump", "Go to page")
                    }),
                ),
                (
                    kobo_sdk::SlotWidth::Fill,
                    Box::new(move |slot| {
                        section(slot, "View")
                            .button("comic-pan", "Move page")
                            .button("comic-options", "Reading options")
                    }),
                ),
            ];
            screen.band(kobo_sdk::BandAlign::Top, slots)
        } else {
            let screen = section(page_size(screen), "Pages")
                .buttons([("comic-pages", "All pages"), ("comic-jump", "Go to page")]);
            section(screen, "View").buttons([
                ("comic-pan", "Move page"),
                ("comic-options", "Reading options"),
            ])
        };
        screen
            .bottom_action(
                if self.save_state == SaveState::Failed {
                    "comic-save-retry"
                } else {
                    "comic-read"
                },
                if self.save_state == SaveState::Failed {
                    "Retry saving"
                } else {
                    "Back to the page"
                },
            )
            .build()
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
                    screen = if self.full_page() && self.mode != Mode::Pan {
                        screen.full_bleed_picture(picture, max_mm)
                    } else {
                        screen.unframed_picture(picture, max_mm)
                    };
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
                screen
                    .page_turns(previous, next)
                    .reading_menu("comic-controls")
                    .build()
                    .with_auto_hidden_top_bar(self.full_page())
            }
            // Grouped by what each control is for: how large the page is
            // drawn, which page it is, and how the comic is read. Eight equal
            // buttons in a block gave the fit, which is a choice of two, the
            // look of two more commands, and the zoom, which is a number, the
            // look of two places to go. Back to the page is the one thing
            // set apart, at the foot. Where the panel is too short for the
            // group names, held sideways at the larger text sizes, the groups
            // keep their order and lose their names.
            Mode::Controls => {
                let shapes = [(true, false), (false, false), (true, true), (false, true)];
                let fits = |screen: &Screen| {
                    !screen
                        .diagnostics(&self.metrics, &measured_chrome(screen))
                        .has_errors()
                };
                shapes
                    .iter()
                    .map(|&(named, columns)| self.controls(named, columns))
                    .find(fits)
                    .unwrap_or_else(|| self.controls(false, true))
            }
            Mode::Details => ScreenBuilder::new("comic-details")
                .top_bar("Comic details")
                .owns_back(true)
                .text(self.notice.as_deref().unwrap_or("No further details."))
                .bottom_action("comic-read", "Read")
                .build(),
            // Each setting is a switch that says what it is and shows
            // whether it is on. Buttons named after the current state read as
            // the thing a tap would do, which was the opposite.
            Mode::Options => {
                let memory = self.reader.memory();
                ScreenBuilder::new("comic-options")
                    .top_bar("Reading options")
                    .owns_back(true)
                    .toggle(
                        "comic-direction",
                        "Read right to left",
                        "For manga and other comics drawn that way",
                        memory.right_to_left,
                    )
                    .toggle(
                        "comic-spreads",
                        "Two-page spreads",
                        "With the page turned sideways. Covers stay on their own.",
                        memory.spreads,
                    )
                    .toggle(
                        "comic-full-page",
                        "Page on its own",
                        "No bar over the page until you tap the top",
                        memory.full_page,
                    )
                    .button("comic-rotate", "Rotate page")
                    .bottom_action("comic-read", "Read")
                    .build()
            }
            Mode::Pages => ScreenBuilder::new("comic-pages")
                .top_bar("Pages")
                .owns_back(true)
                .picture_tiles(
                    TileShape::Portrait,
                    (0..self.previews.len()).map(|index| {
                        (
                            format!("comic-page-{index}"),
                            format!("Page {}", self.preview_start + index + 1),
                            Glyph::Book,
                            self.previews[index],
                        )
                    }),
                )
                .band(
                    kobo_sdk::BandAlign::Middle,
                    [
                        ("comic-thumbs-previous", "Previous", self.preview_start > 0),
                        (
                            "comic-thumbs-next",
                            "Next",
                            self.preview_start + self.thumbs() < self.reader.comic().pages.len(),
                        ),
                    ]
                    .map(|(name, label, enabled)| {
                        (kobo_sdk::SlotWidth::Fill, move |slot: ScreenBuilder| {
                            slot.button_with_state(
                                name,
                                label,
                                if enabled {
                                    kobo_sdk::ControlState::Enabled
                                } else {
                                    kobo_sdk::ControlState::Disabled
                                },
                            )
                        })
                    }),
                )
                .bottom_action("comic-read", "Return to reading")
                .build(),
            Mode::Jump => ScreenBuilder::new("comic-jump")
                .top_bar("Go to page")
                .owns_back(true)
                .compose(|screen| match &self.jump_refused {
                    Some(refused) => {
                        screen.banner(kobo_sdk::BannerLevel::Attention, refused.clone())
                    }
                    None => screen,
                })
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
                self.picture = put_page(context, PAGE, picture);
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
    /// How many pages the page list shows at once: one row of portrait
    /// tiles, as many as the panel fits across.
    fn thumbs(&self) -> usize {
        self.metrics.grid_columns(TileShape::Portrait).clamp(2, 4)
    }

    /// Each page in the list is drawn at the size of its tile. Fitted to a
    /// fixed stamp, they came out a fraction of the tile on a Clara.
    fn preview(&mut self, context: &mut Context) {
        for handle in 0..4 {
            context.drop_picture(PictureHandle(THUMB_BASE + handle));
        }
        self.previews.clear();
        let thumbs = self.thumbs();
        let across = u32::try_from(thumbs).unwrap_or(2);
        let width = u32::try_from(self.metrics.content_width().max(1)).unwrap_or(1) / across;
        let height = width.saturating_mul(3) / 2;
        for index in
            self.preview_start..(self.preview_start + thumbs).min(self.reader.comic().pages.len())
        {
            let handle =
                PictureHandle(THUMB_BASE + u32::try_from(index - self.preview_start).unwrap_or(0));
            let preview = self
                .reader
                .thumbnail_within(index, width, height)
                .ok()
                .and_then(|p| put_page(context, handle, p));
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
                self.jump_refused = Some(format!(
                    "Choose a page from 1 to {}.",
                    self.reader.comic().pages.len()
                ));
                self.number.clear();
            }
            return Outcome::Changed;
        } else {
            return Outcome::Elsewhere;
        }
        self.jump_refused = None;
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
            "comic-full-page",
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
                self.preview_start = self.reader.memory().page / self.thumbs() * self.thumbs();
                self.preview(context);
            }
            "comic-jump" => {
                self.mode = Mode::Jump;
                self.number.clear();
                self.jump_refused = None;
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
            "comic-full-page" => {
                self.reader.memory_mut().full_page = !self.reader.memory().full_page;
            }
            "comic-next" => {
                self.turn(context, true);
                return Outcome::Changed;
            }
            "comic-previous" => {
                self.turn(context, false);
                return Outcome::Changed;
            }
            "comic-thumbs-next"
                if self.preview_start + self.thumbs() < self.reader.comic().pages.len() =>
            {
                self.preview_start += self.thumbs();
                self.preview(context);
            }
            "comic-thumbs-previous" if self.preview_start > 0 => {
                self.preview_start = self.preview_start.saturating_sub(self.thumbs());
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

/// The frame the runtime draws around `screen`, measured with its status
/// strip unless the screen is a reading page, which is drawn without one.
/// Measured with no strip at all, the controls fitted here and ran off the
/// panel on the reader.
fn measured_chrome(screen: &Screen) -> kobo_ui::Chrome {
    if screen.reading {
        kobo_ui::Chrome::for_screen(screen, false, None)
    } else {
        kobo_ui::Chrome::measuring(true)
    }
}

/// Keep RGB only when its bounded wire representation fits. Otherwise reduce
/// its dimensions, preserving aspect ratio and color instead of dropping it.
fn put_page(
    context: &mut Context,
    handle: PictureHandle,
    mut picture: kobo_image::Picture,
) -> Option<TilePicture> {
    if picture.colour().is_some() {
        let max_pixels = kobo_sdk::MAX_PICTURE_BYTES / 3;
        while picture.grey().len() > max_pixels {
            picture = picture
                .fit(
                    (picture.width() * 9 / 10).max(1),
                    (picture.height() * 9 / 10).max(1),
                )
                .ok()?;
        }
        context.put_colour_picture(
            handle,
            picture.width(),
            picture.height(),
            picture.into_colour()?,
        )
    } else {
        context.put_picture(
            handle,
            picture.width(),
            picture.height(),
            picture.into_grey(),
        )
    }
}
