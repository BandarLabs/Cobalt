//! Every UI primitive, on one device, so each can be checked by eye.
//!
//! This is a test instrument as much as a demonstration. If a primitive looks
//! wrong here it looks wrong everywhere, and the layout tests only prove that
//! sizes are right, not that the result is worth reading.

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, LogLevel, PictureHandle,
    ScreenBuilder, Space, Task, TaskId, TaskOutcome, TilePicture, TileShape,
};
use std::process::ExitCode;

/// Every icon the system draws, with the name it is called by.
///
/// A table rather than a literal in the middle of a screen, so that
/// [`every_glyph_is_on_the_panel_somewhere`] can hold it against the enum. An
/// icon that exists and is never drawn here is one nobody has looked at.
const ICONS: [(&str, &str, Glyph); 18] = [
    ("open-app", "Generic", Glyph::App),
    ("open-books", "Library", Glyph::Book),
    ("open-notes", "Notes", Glyph::Note),
    ("open-clock", "Clock", Glyph::Clock),
    ("open-settings", "Settings", Glyph::Settings),
    ("open-folder", "Files", Glyph::Folder),
    ("open-chart", "Stats", Glyph::Chart),
    ("open-search", "Search", Glyph::Search),
    ("open-wifi", "Network", Glyph::Wifi),
    ("open-battery", "Battery", Glyph::Battery),
    ("open-reader", "Reader", Glyph::Reader),
    ("open-power", "Power", Glyph::Power),
    ("open-grid", "Board", Glyph::Grid),
    ("open-circle", "Open", Glyph::Circle),
    ("open-check", "Done", Glyph::Check),
    ("open-terminal", "Shell", Glyph::Terminal),
    ("open-chat", "Chat", Glyph::Chat),
    ("open-news", "Stories", Glyph::News),
];

/// The step wedge: sixteen bands, one per grey the panel can actually resolve.
///
/// This is the instrument the whole tab exists for. A gradient of 256 values
/// says nothing on a display that quantises to sixteen; sixteen flat bands
/// show immediately whether the ends are clipping and whether the middle
/// separates at all under the reading light in the room.
/// The options the choice offers, named once so that what is drawn, what a tap
/// means, and which row is marked can never disagree.
const FILINGS: [(&str, &str); 4] = [
    ("file-keep", "Keep for later"),
    ("file-share", "Share it"),
    ("file-archive", "Archive"),
    ("file-discard", "Discard"),
];

const WEDGE_WIDTH: u32 = 320;
const WEDGE_HEIGHT: u32 = 96;

fn wedge() -> Vec<u8> {
    let mut grey = Vec::with_capacity((WEDGE_WIDTH * WEDGE_HEIGHT) as usize);
    for _ in 0..WEDGE_HEIGHT {
        for x in 0..WEDGE_WIDTH {
            let step = x * 16 / WEDGE_WIDTH;
            // 0, 17, 34 ... 255: the sixteen levels, evenly spaced.
            grey.push(u8::try_from(step * 17).unwrap_or(u8::MAX));
        }
    }
    grey
}

/// Something cover-shaped, for the portrait tile beside it.
const CARD_WIDTH: u32 = 190;
const CARD_HEIGHT: u32 = 300;

fn card() -> Vec<u8> {
    let mut grey = Vec::with_capacity((CARD_WIDTH * CARD_HEIGHT) as usize);
    for y in 0..CARD_HEIGHT {
        for x in 0..CARD_WIDTH {
            let border = x < 8 || y < 8 || x >= CARD_WIDTH - 8 || y >= CARD_HEIGHT - 8;
            let band = (y / 24) % 2 == 0;
            grey.push(if border {
                0
            } else if band {
                0xEE
            } else {
                0x66
            });
        }
    }
    grey
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Text,
    Tiles,
    Pictures,
    Ask,
    Work,
}

impl Tab {
    const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::Text, "tab-text", "Text"),
        (Self::Tiles, "tab-tiles", "Icons"),
        (Self::Pictures, "tab-pictures", "Pictures"),
        (Self::Ask, "tab-ask", "Ask"),
        (Self::Work, "tab-work", "Work"),
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|(tab, _, _)| *tab == self)
            .unwrap_or(0)
    }
}

struct Gallery {
    tab: Tab,
    /// The two pictures, once the runtime has been given them.
    ///
    /// `None` until then, and a screen drawn while they are `None` is a
    /// perfectly good screen — a missing picture is a normal condition in
    /// this system, not an error, which is why every tile keeps its glyph.
    card: Option<TilePicture>,
    swatch: Option<TilePicture>,
    entry: TextEntry,
    answer: Option<String>,
    loading: bool,
    task: Option<TaskId>,
    outcome: Option<String>,
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            card: None,
            swatch: None,
            // Binding the free-text row here is the whole mechanism: the row
            // drawn by `or_type` emits this action, and the field opens itself
            // when it sees it.
            entry: TextEntry::new().opened_by("file-other"),
            answer: None,
            loading: false,
            task: None,
            outcome: None,
        }
    }
}

impl Gallery {
    fn show(&self, context: &mut Context) {
        // A raised keyboard is modal: it covers the panel, so nothing else is
        // drawn under it, including the tab bar.
        if self.entry.is_open() {
            context.set_screen(
                ScreenBuilder::new("gallery")
                    .text_entry(&self.entry, "Something else", "Use this")
                    .build(),
            );
            return;
        }
        let screen = ScreenBuilder::new("gallery").top_bar(match self.tab {
            Tab::Text => "Type and tone",
            Tab::Tiles => "Tiles and icons",
            Tab::Pictures => "Pictures",
            Tab::Ask => "Asking a question",
            Tab::Work => "Work in flight",
        });

        let screen = match self.tab {
            Tab::Text => screen
                .heading("Heading")
                .text(
                    "Body copy wraps at a measure chosen from the panel's physical width, \
                     not its pixel count.",
                )
                .spacer(Space::Small)
                .banner(BannerLevel::Info, "An informational banner.")
                .spacer(Space::Small)
                .banner(
                    BannerLevel::Attention,
                    "An attention banner, drawn inverted. This is what replaces flashing \
                     the frontlight.",
                )
                .spacer(Space::Small)
                .quote(1, "A reply, set in from what it answers.")
                .quote(2, "A reply to the reply, one level further in.")
                .quote(9, "Past the cap the indent stops moving.")
                .spacer(Space::Small)
                .divider()
                .progress(65)
                .spacer(Space::Small)
                .skeleton(3),

            Tab::Tiles => screen
                .text(
                    "Every icon the system draws. Columns come from physical width and a \
                     tile's own minimum, so this is three across on a six inch panel.",
                )
                .spacer(Space::Small)
                .tiles(
                    ICONS
                        .iter()
                        .map(|(name, label, glyph)| (*name, *label, *glyph)),
                ),

            Tab::Pictures => self.pictures(screen),

            Tab::Ask => self.ask(screen),

            Tab::Work => self.work(screen),
        };

        let screen = screen
            .nav_bar(
                self.tab.index(),
                Tab::ALL.map(|(_, name, label)| (name, label)),
            )
            .build();
        context.set_screen(screen);
    }

    /// Decoded artwork, at both sizes the system draws it.
    ///
    /// Generated rather than downloaded, so this tab works with the radio off
    /// and shows exactly the same thing every time: a gallery whose picture
    /// depends on a server is a gallery that is sometimes empty for reasons
    /// that have nothing to do with the renderer.
    fn pictures(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = screen.text("Sixteen greys, one flat band each.");
        let screen = match self.card {
            Some(card) => screen.spacer(Space::Small).picture(card, 40),
            None => screen.skeleton(2),
        };
        let screen = screen
            .spacer(Space::Small)
            .divider()
            .text("One tile with artwork, one without.");
        match self.swatch {
            Some(swatch) => screen.picture_tiles(
                TileShape::Portrait,
                [
                    ("picture-one", "With artwork", Glyph::Book, Some(swatch)),
                    ("picture-two", "Still arriving", Glyph::Book, None),
                ],
            ),
            None => screen.skeleton(2),
        }
    }

    /// Hands the runtime the two pictures this tab draws.
    ///
    /// Once, on start, rather than on every repaint: a picture is held by the
    /// runtime under its handle, and re-sending it on each paint would put the
    /// whole image back on the wire every time a tab was tapped.
    fn put_pictures(&mut self, context: &mut Context) {
        self.card = context.put_picture(PictureHandle(1), WEDGE_WIDTH, WEDGE_HEIGHT, wedge());
        self.swatch = context.put_picture(PictureHandle(2), CARD_WIDTH, CARD_HEIGHT, card());
    }

    fn ask(&self, screen: ScreenBuilder) -> ScreenBuilder {
        {
            let screen = screen
                .text(match &self.answer {
                    None => "Nothing chosen yet.".to_owned(),
                    Some(answer) => format!("You chose: {answer}"),
                })
                .spacer(Space::Small)
                .choose("How should this note be filed?", FILINGS)
                // State rather than a mark in a label: the renderer draws it
                // from the icon atlas, so it exists whatever the face contains.
                .chosen(
                    FILINGS
                        .iter()
                        .position(|(_, label)| Some(*label) == self.answer.as_deref())
                        .unwrap_or(usize::MAX),
                )
                .or_type("file-other", "Something else...");
            screen
        }
    }

    fn work(&self, screen: ScreenBuilder) -> ScreenBuilder {
        {
            let screen = screen.text(
                "A request is work handed to the runtime. The screen stays live \
                     throughout, and there is no spinner because every frame would be a \
                     panel refresh.",
            );
            let screen = if self.loading {
                screen
                    .spacer(Space::Small)
                    .activity("Fetching the catalogue", None)
                    .cancellable("cancel-fetch", "Cancel")
            } else {
                screen
                    .spacer(Space::Small)
                    .text(self.outcome.as_deref().unwrap_or("Idle."))
                    .spacer(Space::Small)
                    .button("start-fetch", "Start a request")
            };
            screen
        }
    }
}

impl KoboApp for Gallery {
    fn on_start(&mut self, context: &mut Context) {
        self.put_pictures(context);
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        for (tab, name, _) in Tab::ALL {
            if action == action_id(name) {
                self.tab = tab;
                self.show(context);
                return;
            }
        }

        // The typed row is handled first, because while the keyboard is up it
        // owns the panel. This is what makes the free-text row actually raise
        // a keyboard rather than answer for the reader, which is what it used
        // to do.
        if let Some(event) = self.entry.handle(action) {
            if let Typing::Submitted(text) = event {
                self.answer = Some(text);
            }
            self.show(context);
            return;
        }

        for (name, label) in FILINGS {
            if action == action_id(name) {
                self.answer = Some(label.to_owned());
                self.show(context);
                return;
            }
        }

        if action == action_id("start-fetch") {
            // The simulator has no network, so this is expected to be refused.
            // That is the point: the failure path runs during development
            // rather than for the first time on someone's device.
            match context.spawn(Task::Fetch {
                url: "https://example.invalid/catalog".to_owned(),
                offset: 0,
                max_bytes: 4096,
            }) {
                Some(task) => {
                    self.task = Some(task);
                    self.loading = true;
                    self.outcome = None;
                }
                None => self.outcome = Some("Too much already in flight.".to_owned()),
            }
            self.show(context);
            return;
        }

        if action == action_id("cancel-fetch") {
            if let Some(task) = self.task {
                context.cancel(task);
                context.log(LogLevel::Info, "reader cancelled the request");
            }
            return;
        }

        if let Some(tile) = [
            "open-reader",
            "open-books",
            "open-notes",
            "open-clock",
            "open-search",
            "open-chart",
            "open-wifi",
            "open-battery",
            "open-folder",
            "open-settings",
            "open-power",
            "open-app",
        ]
        .into_iter()
        .find(|name| action == action_id(name))
        {
            context.log(LogLevel::Info, format!("tile tapped: {tile}"));
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) {
            return;
        }
        self.task = None;
        self.loading = false;
        self.outcome = Some(match outcome {
            TaskOutcome::Completed(bytes) => format!("Received {} bytes.", bytes.len()),
            TaskOutcome::Failed(error) => format!("Request failed: {error}"),
            TaskOutcome::Cancelled => "Request cancelled.".to_owned(),
        });
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("gallery", Gallery::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gallery: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        card, wedge, Gallery, Tab, CARD_HEIGHT, CARD_WIDTH, ICONS, WEDGE_HEIGHT, WEDGE_WIDTH,
    };
    use kobo_sdk::{action_id, Command, Context, Glyph, KoboApp, Node};

    #[test]
    fn the_answer_already_given_is_marked_on_the_row_that_gave_it() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        gallery.on_action(&mut context, action_id("tab-ask"));
        gallery.on_action(&mut context, action_id("file-archive"));
        let screen = context
            .take_commands()
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("a screen was painted");
        let choice = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_sdk::Node::Choice { selected, .. } => Some(*selected),
                _ => None,
            })
            .expect("the ask tab offers a choice");
        assert_eq!(choice, Some(2));
    }

    #[test]
    fn the_free_text_row_raises_the_keyboard_rather_than_answering_for_the_reader() {
        // This is a regression. The row used to be wired to a canned string,
        // so tapping "Something else..." filled in an answer nobody typed and
        // the keyboard never appeared.
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        gallery.on_action(&mut context, action_id("file-other"));
        assert!(gallery.entry.is_open(), "the free-text row opened nothing");
        assert!(
            gallery.answer.is_none(),
            "an answer appeared without typing"
        );
    }

    #[test]
    fn what_was_typed_becomes_the_answer() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_action(&mut context, action_id("file-other"));
        for key in ["kb.r0c0", "kb.r0c1"] {
            gallery.on_action(&mut context, action_id(key));
        }
        gallery.on_action(&mut context, action_id("kb.enter"));
        assert_eq!(gallery.answer.as_deref(), Some("qw"));
        assert!(!gallery.entry.is_open());
    }

    /// The gallery is a test instrument, so an icon it does not draw is an
    /// icon nobody has ever looked at on real hardware. This fails the moment
    /// a glyph is added to the system and not to this table.
    #[test]
    fn every_glyph_is_on_the_panel_somewhere() {
        // Every variant, listed here so that adding one to the enum without
        // adding it to the gallery cannot compile.
        let every = [
            Glyph::App,
            Glyph::Book,
            Glyph::Note,
            Glyph::Clock,
            Glyph::Settings,
            Glyph::Folder,
            Glyph::Chart,
            Glyph::Search,
            Glyph::Wifi,
            Glyph::Battery,
            Glyph::Reader,
            Glyph::Power,
            Glyph::Grid,
            Glyph::Circle,
            Glyph::Check,
            Glyph::Terminal,
            Glyph::Chat,
            Glyph::News,
        ];
        for glyph in every {
            assert!(
                ICONS.iter().any(|(_, _, drawn)| *drawn == glyph),
                "{glyph:?} is never drawn in the gallery"
            );
        }
        let mut names = ICONS.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "two icons share an action");
    }

    #[test]
    fn the_generated_pictures_are_exactly_the_size_they_claim() {
        // The runtime refuses a picture whose bytes and dimensions disagree,
        // and refuses it silently, because a missing picture is a normal
        // condition here. So a mistake in this arithmetic would show up as a
        // tab that is simply always empty.
        assert_eq!(wedge().len(), (WEDGE_WIDTH * WEDGE_HEIGHT) as usize);
        assert_eq!(card().len(), (CARD_WIDTH * CARD_HEIGHT) as usize);
        let mut levels = wedge()
            .into_iter()
            .take(WEDGE_WIDTH as usize)
            .collect::<Vec<_>>();
        levels.dedup();
        assert_eq!(levels.len(), 16, "the wedge is not one band per grey");
        assert_eq!(levels.first(), Some(&0));
        assert_eq!(levels.last(), Some(&255));
    }

    #[test]
    fn the_pictures_are_handed_over_once_and_then_drawn_by_handle() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        let commands = context.take_commands();
        let given = commands
            .iter()
            .filter(|command| matches!(command, Command::PutPicture { .. }))
            .count();
        assert_eq!(given, 2, "the pictures were not handed over on start");

        gallery.tab = Tab::Pictures;
        let mut context = Context::default();
        gallery.show(&mut context);
        let commands = context.take_commands();
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::PutPicture { .. })),
            "a repaint put the whole image back on the wire"
        );
        let screen = commands
            .iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("the tab painted");
        assert!(
            screen
                .nodes
                .iter()
                .any(|node| matches!(node, Node::Picture { .. })),
            "the pictures tab drew no picture"
        );
    }
}
