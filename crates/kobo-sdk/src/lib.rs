#![forbid(unsafe_code)]

//! Plain Rust application API for producing Kobo UI commands.
//!
//! Applications own their state and call [`AppRunner::start`] and
//! [`AppRunner::action`] from their platform event loop.

pub use kobo_protocol::{
    is_valid_key, AppInfo, AppLinkState, AudioPlaybackState, AudioSource, BatteryDetail,
    BluetoothDevice, BluetoothDeviceKind, Credential, DenyReason, DeviceError, DeviceIdentity,
    DeviceRequest, DeviceResult, DictionaryEntry, Frame, Header, Lifecycle, LogLevel, Message,
    RemoteInstallOutcome, SecretHeader, ShellError, ShellEvent, ShellRequest, StoreError,
    StoreRequest, StoreResult, StreamError, Task, TaskError, TaskId, TaskOutcome, UpdateChannel,
    WifiNetwork, CACHE_PREFIX, MAX_CACHE_KEYS, MAX_FONT_BYTES, MAX_HEADERS, MAX_HEADER_NAME,
    MAX_HEADER_VALUE, MAX_INLINE_PICTURE_BYTES, MAX_LOOKUP_WORD_BYTES, MAX_PICTURE_BYTES,
    MAX_PICTURE_CHUNK_BYTES, MAX_RADIO_DEVICES, MAX_RADIO_NAME, MAX_SHELF_CHUNK, MAX_SHELL_CHUNK,
    MAX_STORE_KEYS, MAX_STORE_VALUE, MAX_TASK_BYTES, MAX_URL_LEN,
};
pub use kobo_ui::QuoteRole;
pub use kobo_ui::{
    terminal_grid, terminal_grid_for, typographic_cover, ActionId, BandAlign, BandSlot,
    BannerLevel, BarAction, BarStyle, BottomAction, Caret, Cell, Chip, Chrome, ControlState,
    DiagnosticSeverity, DisplayMetrics, Emphasis, Fold, FontHandle, Freeform, Glyph, InlineFormula,
    LayoutIssue, LayoutIssueKind, NavBar, Node, NodeId, Overlay, OverlayKind, ParagraphAlignment,
    ParagraphPresentation, Percent, PictureHandle, ProseArea, RichTextSpan, Row, RowLead, RowState,
    Screen, SlotWidth, Space, TextHit, TextPresentation, TextSelection, Tile, TilePicture,
    TileShape, TileState, TopBar, TransferFailure, CLARA_BW_METRICS, MAX_BAND_SLOTS, MAX_CELLS,
    MAX_CHIPS, MAX_CHOICE_OPTIONS, MAX_COLUMNS, MAX_INLINE_FORMULAE, MAX_QUOTE_DEPTH, MAX_ROWS,
    MAX_TABS, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS, TILE_BADGE_LIMIT,
};
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fmt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// The capability and power model shared with the runtime.
pub use kobo_policy as permissions;

pub use kobo_policy::{Capability, Declared, Grant, Grants, PowerPolicy};

pub mod audio;
pub mod credentials;
/// Common application and builder types.
pub mod keyboard;
pub mod terminal;

pub use audio::{AudioMetadata, AudioPlayer};

pub mod prelude {
    pub use crate::{
        action_id, ActionId, AppIcon, AppLinkState, AppMetadata, AppRunner, AppShelf, AppShell,
        AppStore, AudioMetadata, AudioPlaybackState, AudioPlayer, AudioSource, BluetoothDevice,
        BluetoothDeviceKind, Capability, Client, ClientEvent, Command, Context, ControlState,
        DenyReason, Device, DeviceError, DeviceIdentity, DeviceRequest, DeviceResult, DialogAction,
        Failure, Grant, Grants, Heartbeat, KoboApp, Lifecycle, Navigator, Node, NodeId,
        PowerPolicy, RemoteInstallOutcome, Screen, ScreenBuilder, ShelfDownload, ShelfProgress,
        ShelfUpload, ShellError, ShellEvent, ShellRequest, StandardState, StoreError, StoreRequest,
        StoreResult, WifiNetwork,
    };
}

/// A small, typed back stack for application-owned destinations.
///
/// The root can never be popped, so an action handler can call [`Self::back`]
/// without manufacturing an impossible "no current screen" case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Navigator<Route> {
    root: Route,
    stack: Vec<Route>,
}

impl<Route> Navigator<Route> {
    #[must_use]
    pub fn new(root: Route) -> Self {
        Self {
            root,
            stack: Vec::new(),
        }
    }

    #[must_use]
    pub fn current(&self) -> &Route {
        self.stack.last().unwrap_or(&self.root)
    }

    #[must_use]
    pub fn can_go_back(&self) -> bool {
        !self.stack.is_empty()
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len() + 1
    }

    pub fn push(&mut self, route: Route) {
        self.stack.push(route);
    }

    /// Replaces the current destination without adding a back-stack entry.
    pub fn replace(&mut self, route: Route) {
        if let Some(current) = self.stack.last_mut() {
            *current = route;
        } else {
            self.root = route;
        }
    }

    /// Returns to the previous destination, or leaves the root unchanged.
    #[must_use]
    pub fn back(&mut self) -> bool {
        if self.can_go_back() {
            self.stack.pop();
            true
        } else {
            false
        }
    }

    pub fn reset(&mut self, root: Route) {
        self.stack.clear();
        self.root = root;
    }
}

/// An application icon that remains legible when an image is unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppIcon {
    Glyph(Glyph),
    Picture {
        picture: TilePicture,
        fallback: Glyph,
    },
}

impl AppIcon {
    #[must_use]
    pub const fn glyph(glyph: Glyph) -> Self {
        Self::Glyph(glyph)
    }

    #[must_use]
    pub const fn picture(picture: TilePicture, fallback: Glyph) -> Self {
        Self::Picture { picture, fallback }
    }

    fn tile(self, action: ActionId, label: impl Into<String>) -> Tile {
        match self {
            Self::Glyph(glyph) => Tile::new(action, label, glyph),
            Self::Picture { picture, fallback } => {
                Tile::new(action, label, fallback).with_picture(picture)
            }
        }
    }
}

/// Compile-time application identity and launcher presentation.
///
/// Keeping this borrowed makes a manifest usable as a `const`, while
/// [`Self::tile`] turns it directly into the SDK's launcher primitive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppMetadata {
    pub id: &'static str,
    pub display_name: &'static str,
    pub summary: &'static str,
    pub icon: AppIcon,
}

impl AppMetadata {
    #[must_use]
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        summary: &'static str,
        icon: AppIcon,
    ) -> Self {
        Self {
            id,
            display_name,
            summary,
            icon,
        }
    }

    #[must_use]
    pub fn tile(self, action: ActionId) -> Tile {
        self.icon.tile(action, self.display_name)
    }
}

/// Standard whole-screen conditions with consistent titles and urgency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandardState {
    Empty,
    Offline,
    PermissionDenied,
    Error,
}

impl StandardState {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Empty => "Nothing here yet",
            Self::Offline => "You're offline",
            Self::PermissionDenied => "Permission needed",
            Self::Error => "Something went wrong",
        }
    }

    /// The mark drawn above the title.
    ///
    /// A state screen has four words on it and a thousand pixels of nothing
    /// under them; the mark is what makes it read as a considered page rather
    /// than a page that failed to load.
    const fn glyph(self) -> Glyph {
        match self {
            Self::Empty => Glyph::Circle,
            Self::Offline => Glyph::Wifi,
            // A key, not a person. The commonest way to reach this state is an
            // API key the reader has not installed, and a head and shoulders
            // sends whoever is looking at it hunting for an account setting
            // that does not exist.
            Self::PermissionDenied => Glyph::Key,
            Self::Error => Glyph::Close,
        }
    }
}

/// The application the reader is sent to when a failure is a missing network.
const NETWORK_SETTINGS_APP: &str = "settings";

/// The reserved action that takes a reader from a failure to the Wi-Fi screen.
///
/// Handled by [`AppRunner`] before the application sees it, so an application
/// gets the route by naming the action and never writes a line about it. The
/// name is prefixed because it is the SDK's, not the application's, and an
/// application that happens to call something "wifi" must not collide with it.
///
/// This exists because the advice for [`TaskError::Offline`] ends "Join Wi-Fi
/// and try again" and there was no way to do the first half of that. Every
/// networked application in the tree told the reader to join a network and
/// then offered a single control that retried the thing that had just failed.
/// Getting to Wi-Fi meant leaving for the Kobo reader, coming back through the
/// launcher and into Settings, which is four screens to answer a sentence the
/// application itself put on the panel.
pub const JOIN_WIFI: &str = "kobo.join-wifi";

/// What to put in front of a reader when a task failed.
///
/// One mapping, here, rather than a `match` on [`TaskError`] copied into every
/// application. Five applications wrote five different sentences for the same
/// failure, and adding a variant to `TaskError` broke all of them at once,
/// which is how this came to exist.
///
/// The wording assumes the SDK has already done what it can. By the time an
/// application sees a failure, [`Context::spawn_retrying`] has taken its
/// second attempt, so the advice can be plain rather than hedged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Failure {
    /// The whole-screen state, for when there is nothing on the page already.
    pub state: StandardState,
    /// One sentence saying what happened and what would change it. Short
    /// enough to sit in a banner over content that is already on screen.
    pub advice: &'static str,
    /// Whether offering a Retry control is honest.
    ///
    /// A refused permission and a body over the ceiling will do the same thing
    /// next time, and a control that cannot help is worse than no control.
    pub retryable: bool,
}

impl Failure {
    /// Reads a task failure the way a reader would.
    #[must_use]
    pub const fn of(error: TaskError) -> Self {
        match error {
            TaskError::Offline => Self {
                state: StandardState::Offline,
                advice: "This reader is not on a network. Join Wi-Fi and try again.",
                retryable: true,
            },
            TaskError::Unreachable => Self {
                state: StandardState::Error,
                advice: "The service did not answer. It may be down.",
                retryable: true,
            },
            TaskError::TimedOut => Self {
                state: StandardState::Error,
                advice: "The network was too slow to answer.",
                retryable: true,
            },
            TaskError::Denied => Self {
                state: StandardState::PermissionDenied,
                advice: "This application is not allowed to do that.",
                retryable: false,
            },
            // The host's refusal rather than this device's, so the advice
            // points at the service. Retrying is pointless until whoever
            // holds the reader has an account there, and saying "try again"
            // would send them round the same loop.
            TaskError::Unauthorized => Self {
                state: StandardState::PermissionDenied,
                advice: "This service will not answer without an account.",
                retryable: false,
            },
            // Names the supported way to fix it rather than a path. The path
            // wrapped mid-directory on the panel, and pointing at a file tells
            // whoever is holding the reader to go and edit one by hand when
            // there is a command that does it over Wi-Fi.
            TaskError::NoCredential => Self {
                state: StandardState::PermissionDenied,
                advice: "This reader has no API key for that service. \
                         Install one with kobo secret set.",
                retryable: false,
            },
            TaskError::TooLarge => Self {
                state: StandardState::Error,
                advice: "The reply was too large to read on this device.",
                retryable: false,
            },
            TaskError::NotFound => Self {
                state: StandardState::Empty,
                advice: "The service had nothing to return.",
                retryable: false,
            },
        }
    }

    /// The heading for a whole-screen version of this failure.
    #[must_use]
    pub const fn title(self) -> &'static str {
        self.state.title()
    }

    /// The advice, naming the credential the work asked for.
    ///
    /// [`Failure::of`] is const and its advice is a `&'static str`, so it can
    /// only say "that service". An application that runs against three
    /// providers then tells whoever is holding the reader to install a key
    /// without saying which one, and they have to guess or go and read the
    /// source. The application knows the name, because it named the secret
    /// when it spawned the work, so it is the one that can say it.
    ///
    /// Every other failure is unchanged: a slow network and a refused request
    /// have nothing to do with which key was asked for.
    #[must_use]
    pub fn naming(self, secret: &str) -> String {
        if self.state == StandardState::PermissionDenied
            && self.advice.starts_with("This reader has no API key")
        {
            return format!(
                "This reader has no API key called {secret}. \
                 Install one with kobo secret set {secret}."
            );
        }
        self.advice.to_owned()
    }
}

impl Failure {
    /// Reads a shelf write failure the way a reader would.
    ///
    /// The companion to [`Failure::of`], for the other error type an
    /// application can be handed. [`StoreError`]'s own `Display` is a
    /// diagnostic fragment, so an application that puts it on screen shows a
    /// sentence starting in lower case that names none of the things the
    /// person could do about it.
    ///
    /// None of these is retryable. A full card and a rejected key are both the
    /// same the second time, and the two that are not the application's fault
    /// still need a human to clear space.
    #[must_use]
    pub const fn storing(error: StoreError) -> Self {
        match error {
            StoreError::NoRoom | StoreError::TooFull => Self {
                state: StandardState::Error,
                advice: "There is not enough room left on this reader. \
                         Delete something and try again.",
                retryable: false,
            },
            StoreError::Unwritable => Self {
                state: StandardState::Error,
                advice: "This reader would not save the file.",
                retryable: false,
            },
            StoreError::Missing => Self {
                state: StandardState::Empty,
                advice: "That file is no longer on this reader.",
                retryable: false,
            },
            // Reachable only from a name the application itself built, so it
            // is a bug in the application rather than anything the reader did.
            StoreError::BadKey => Self {
                state: StandardState::Error,
                advice: "This application asked for a file name the reader will not accept.",
                retryable: false,
            },
        }
    }
}

/// One action in a standard confirmation screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DialogAction {
    pub name: String,
    pub label: String,
    pub state: ControlState,
}

impl DialogAction {
    #[must_use]
    pub fn new(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            state: ControlState::Enabled,
        }
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.state = if disabled {
            ControlState::Disabled
        } else {
            ControlState::Enabled
        };
        self
    }
}

/// Builds a retained screen with deterministic identifiers.
///
/// Node identifiers are allocated in declaration order. Action identifiers are
/// derived from their string names, so applications can dispatch actions with
/// [`action_id`] without retaining a builder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenBuilder {
    id: u32,
    next_node: u32,
    top_bar: Option<TopBar>,
    nodes: Vec<Node>,
    nav_bar: Option<NavBar>,
    bottom_action: Option<BottomAction>,
    page_turns: Option<kobo_ui::PageTurns>,
    hold: Option<ActionId>,
    owns_back: bool,
    text_scale: Option<kobo_ui::TextScale>,
    overlay: Option<Box<Overlay>>,
    reading: bool,
    reading_font: Option<FontHandle>,
    actions: Vec<(String, ActionId)>,
    warnings: Vec<LayoutIssue>,
}

impl ScreenBuilder {
    #[must_use]
    pub fn new(name: impl AsRef<str>) -> Self {
        Self {
            id: stable_id(name.as_ref()),
            next_node: 1,
            top_bar: None,
            nodes: Vec::new(),
            nav_bar: None,
            bottom_action: None,
            page_turns: None,
            hold: None,
            owns_back: false,
            text_scale: None,
            overlay: None,
            reading: false,
            reading_font: None,
            actions: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[must_use]
    pub fn heading(self, text: impl Into<String>) -> Self {
        self.heading_at_level(1, text)
    }

    /// A heading at a given depth in a document's hierarchy, counting from
    /// one.
    ///
    /// A screen has one heading and calls [`Self::heading`]. This is for
    /// prose that carries real structure -- a book, a paper -- where setting
    /// every level as display type gives a page several titles and no
    /// hierarchy.
    #[must_use]
    pub fn heading_at_level(mut self, level: u8, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Heading {
            id,
            text: text.into(),
            level: level.max(1),
        });
        self
    }

    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Text {
            id,
            text: text.into(),
            links: Vec::new(),
        });
        self
    }

    /// Adds publisher-styled book prose without exposing arbitrary geometry.
    #[must_use]
    pub fn rich_text(
        mut self,
        text: impl Into<String>,
        spans: Vec<RichTextSpan>,
        presentation: ParagraphPresentation,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::RichText {
            id,
            text: text.into(),
            spans,
            links: Vec::new(),
            presentation,
            selection: None,
            formulae: Vec::new(),
        });
        self
    }

    /// Publisher-styled prose with tappable inline destinations.
    #[must_use]
    pub fn rich_text_linking<I, N>(
        mut self,
        text: impl Into<String>,
        spans: Vec<RichTextSpan>,
        presentation: ParagraphPresentation,
        links: I,
    ) -> Self
    where
        I: IntoIterator<Item = (N, usize, usize)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let text = text.into();
        let links = links
            .into_iter()
            .take(kobo_ui::MAX_TEXT_LINKS)
            .filter_map(|(name, start, end)| {
                (start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end))
                .then(|| kobo_ui::TextLink {
                    action: self.register(name.as_ref()),
                    start,
                    end,
                })
            })
            .collect();
        self.nodes.push(Node::RichText {
            id,
            text,
            spans,
            links,
            presentation,
            selection: None,
            formulae: Vec::new(),
        });
        self
    }

    /// Publisher-styled reading prose whose words can be resolved on a hold.
    #[must_use]
    pub fn selectable_rich_text_linking<I, N>(
        mut self,
        text: impl Into<String>,
        spans: Vec<RichTextSpan>,
        presentation: ParagraphPresentation,
        context: u64,
        offset: u32,
        links: I,
    ) -> Self
    where
        I: IntoIterator<Item = (N, usize, usize)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let text = text.into();
        let links = links
            .into_iter()
            .take(kobo_ui::MAX_TEXT_LINKS)
            .filter_map(|(name, start, end)| {
                (start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end))
                .then(|| kobo_ui::TextLink {
                    action: self.register(name.as_ref()),
                    start,
                    end,
                })
            })
            .collect();
        self.nodes.push(Node::RichText {
            id,
            text,
            spans,
            links,
            presentation,
            selection: Some(kobo_ui::TextSelection { context, offset }),
            formulae: Vec::new(),
        });
        self
    }

    /// Sets typeset formulas into the paragraph just added.
    ///
    /// Separate from the calls that add the paragraph because mathematics is
    /// rare and those calls already take everything a paragraph normally has.
    /// Each formula names a picture the application has handed over and the
    /// half-open range of the paragraph's own bytes it is drawn over -- the
    /// written form of the formula, which stays in the text so that a search
    /// still finds it and a reader without the picture still reads it.
    ///
    /// Does nothing if the last thing added was not a paragraph, or if a
    /// range does not land on a character boundary of it.
    #[must_use]
    pub fn with_formulae(mut self, formulae: impl IntoIterator<Item = InlineFormula>) -> Self {
        let Some(Node::RichText {
            text, formulae: on, ..
        }) = self.nodes.last_mut()
        else {
            return self;
        };
        for formula in formulae.into_iter().take(kobo_ui::MAX_INLINE_FORMULAE) {
            if formula.start < formula.end
                && formula.end <= text.len()
                && text.is_char_boundary(formula.start)
                && text.is_char_boundary(formula.end)
                && on
                    .last()
                    .is_none_or(|last: &InlineFormula| last.end <= formula.start)
            {
                on.push(formula);
            }
        }
        self
    }

    /// A paragraph with runs inside it that go somewhere.
    ///
    /// Each link is an action name and the half-open range of the paragraph's
    /// own bytes that names it. Ranges rather than the words themselves,
    /// because a paragraph often says the same words twice and only one of
    /// them is the link; a caller that has the words rather than the offsets
    /// should use `str::find` on the paragraph it is about to pass in, and
    /// leave out anything it cannot locate.
    ///
    /// A range outside the text, or landing inside a character, is dropped
    /// rather than drawn somewhere approximate: a link in the wrong place is
    /// worse than a link that is only in the list.
    #[must_use]
    pub fn text_linking<I, N>(mut self, text: impl Into<String>, links: I) -> Self
    where
        I: IntoIterator<Item = (N, usize, usize)>,
        N: AsRef<str>,
    {
        let id = self.next_id();
        let text = text.into();
        let mut runs = Vec::new();
        let mut source = links.into_iter();
        for (name, start, end) in source.by_ref().take(kobo_ui::MAX_TEXT_LINKS) {
            if start >= end
                || end > text.len()
                || !text.is_char_boundary(start)
                || !text.is_char_boundary(end)
            {
                continue;
            }
            runs.push(kobo_ui::TextLink {
                action: self.register(name.as_ref()),
                start,
                end,
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "text links", kobo_ui::MAX_TEXT_LINKS);
        }
        self.nodes.push(Node::Text {
            id,
            text,
            links: runs,
        });
        self
    }

    /// Adds a line about the content rather than the content itself.
    ///
    /// A date, an author, a size, a count, a status. Set smaller and lighter
    /// than body text, which is what lets a list be read by scanning titles.
    /// Use it for anything that would otherwise be a parenthetical.
    #[must_use]
    pub fn secondary(mut self, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Secondary {
            id,
            text: text.into(),
        });
        self
    }

    /// Names the group of blocks that follows it.
    ///
    /// The organising primitive. [`Self::heading`] is display type belonging to
    /// the *screen*, so using it for a group gives a screen four titles and no
    /// hierarchy; a section is quieter than the heading on purpose and never
    /// competes with it. Every application was building this out of a spacer, a
    /// divider and a line of prose, and getting a slightly different answer.
    ///
    /// The words are used as they are given. Setting a section in capitals is a
    /// house style that breaks on scripts with no case at all, so if capitals
    /// are wanted, supply them.
    #[must_use]
    pub fn section(mut self, title: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Section {
            id,
            title: title.into(),
            value: None,
            link: None,
        });
        self
    }

    /// The same, with a count or a total against the right margin.
    ///
    /// The value is measured first and the title clamped against what is left,
    /// so a long name gives up its own hairline rather than pushing the total
    /// off the panel.
    #[must_use]
    pub fn section_with_value(
        mut self,
        title: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Section {
            id,
            title: title.into(),
            value: Some(value.into()),
            link: None,
        });
        self
    }

    /// Adds a caption-sized trailing destination to the most recent section.
    ///
    /// This deliberately searches backwards so it can follow `section_rows`:
    /// rows remain the content introduced by the section, not an obstacle to
    /// giving that section a "View all" destination.
    #[must_use]
    pub fn section_link(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Section { link, .. }) = self
            .nodes
            .iter_mut()
            .rev()
            .find(|node| matches!(node, Node::Section { .. }))
        {
            *link = Some(BarAction::new(action, label));
        }
        self
    }

    /// Sets a block of labelled facts about the thing on the screen.
    ///
    /// The answer to a detail screen with a dozen things to say and only
    /// [`Self::secondary`] to say them with, which stacks a dozen grey
    /// paragraphs and reads as a page that failed to finish loading.
    ///
    /// Labels share one column measured across every entry at once, so the
    /// values line up; the column is capped so one long label cannot squeeze
    /// every value into a gutter. Entries past `MAX_FACTS` are dropped and
    /// reported by [`Screen::validate`], rather than silently set and clipped.
    #[must_use]
    pub fn facts<I, K, V>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let id = self.next_id();
        let entries = entries
            .into_iter()
            .map(|(label, value)| (label.into(), value.into()))
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            self.nodes.push(Node::Facts { id, entries });
        }
        self
    }

    /// Places two or three columns beside each other.
    ///
    /// The one escape from the downward flow, and deliberately a small one.
    /// Each slot is built with the same builder the screen uses, so ids and
    /// action names carry straight on: a control inside a band is named and
    /// read exactly like a control anywhere else.
    ///
    /// Slots past [`MAX_BAND_SLOTS`] are dropped. When the panel cannot give
    /// every slot a readable width the band stacks itself, so this is always
    /// safe to reach for -- there is no narrow device on which it produces a
    /// column four characters wide.
    ///
    /// ```ignore
    /// screen.band(BandAlign::Top, [
    ///     (SlotWidth::Fixed(300), |slot| slot.picture(cover, 30)),
    ///     (SlotWidth::Fill, |slot| {
    ///         slot.heading(&book.title).secondary(&book.author)
    ///     }),
    /// ])
    /// ```
    #[must_use]
    pub fn band<I, F>(mut self, align: BandAlign, slots: I) -> Self
    where
        I: IntoIterator<Item = (SlotWidth, F)>,
        F: FnOnce(Self) -> Self,
    {
        let id = self.next_id();
        let outer = std::mem::take(&mut self.nodes);
        let mut built = Vec::new();
        let mut done = self;
        for (width, build) in slots.into_iter().take(MAX_BAND_SLOTS) {
            done = build(done);
            let nodes = std::mem::take(&mut done.nodes);
            built.push(BandSlot::new(width, nodes));
        }
        done.nodes = outer;
        if !built.is_empty() {
            done.nodes.push(Node::Band {
                id,
                align,
                slots: built,
            });
        }
        done
    }

    /// Runs a reusable piece of screen without breaking the builder chain.
    ///
    /// Composites are already expressible as plain `fn(ScreenBuilder) ->
    /// ScreenBuilder` functions, and several applications write them, but
    /// calling one meant stopping mid-chain and naming a temporary. This is
    /// the same thing the overlay and band closures do, exposed so anything
    /// can be factored out and reused rather than copied.
    #[must_use]
    pub fn compose(self, build: impl FnOnce(Self) -> Self) -> Self {
        build(self)
    }

    /// Puts a picture beside what it is a picture of.
    ///
    /// The masthead of a details page: a cover on the leading edge, and title,
    /// author and a few facts stacked beside it. There is deliberately no
    /// `Node::Hero` behind this. A hero is a picture next to a column, which
    /// is exactly what [`Self::band`] already is, and neither `SwiftUI` nor
    /// Compose ships a hero primitive either -- both compose one out of a
    /// stack. Adding a node would have meant a layout arm, a draw arm, a
    /// validate arm, three protocol arms and a roundtrip fixture for a screen
    /// that can already be written.
    ///
    /// The picture slot is a physical width, so the cover is the same size on
    /// a Clara as on a Sage. When the panel is too narrow to keep both slots
    /// readable the band stacks them on its own, which is why `width_mm` is
    /// the only measurement here and there is no breakpoint to get wrong.
    ///
    /// `picture` may be `None` -- a catalogue is full of books whose cover has
    /// not arrived, or has arrived and failed to decode -- in which case the
    /// metadata simply takes the whole width rather than sitting beside a
    /// grey rectangle apologising for itself.
    #[must_use]
    pub fn hero<I, K, V>(
        self,
        picture: Option<TilePicture>,
        width_mm: u16,
        title: impl Into<String>,
        subtitle: Option<String>,
        facts: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let title = title.into();
        let facts = facts
            .into_iter()
            .map(|(label, value)| (label.into(), value.into()))
            .collect::<Vec<_>>();
        let metadata = move |builder: Self| {
            let builder = builder.heading(title);
            let builder = match subtitle {
                Some(subtitle) => builder.secondary(subtitle),
                None => builder,
            };
            builder.facts(facts)
        };
        let Some(picture) = picture else {
            return self.compose(metadata);
        };
        self.band(
            BandAlign::Top,
            vec![
                (
                    SlotWidth::Fixed(width_mm.saturating_mul(10)),
                    // Twice the slot width as a height ceiling, so the fixed
                    // width is what actually decides the size of an ordinary
                    // portrait cover while a freak panorama is still stopped
                    // from taking the whole panel.
                    Box::new(move |builder: Self| {
                        builder.picture(picture, width_mm.saturating_mul(2))
                    }) as Box<dyn FnOnce(Self) -> Self>,
                ),
                (SlotWidth::Fill, Box::new(metadata)),
            ],
        )
    }

    /// Asks a question that has to be answered before anything else happens.
    ///
    /// A modal rather than a popover, deliberately: an outside tap does not
    /// close this one, because "did you mean to delete it" answered by
    /// accidentally brushing the panel is not an answer. The affirmative is
    /// the filled control and comes first, the way out is plain and second.
    ///
    /// Every application that deletes, unfollows or overwrites something was
    /// about to build this by hand out of `modal` plus two buttons, and they
    /// would have disagreed about which one was filled.
    #[must_use]
    pub fn confirm(
        self,
        title: impl Into<String>,
        question: impl Into<String>,
        confirm: (impl AsRef<str>, impl Into<String>),
        cancel: (impl AsRef<str>, impl Into<String>),
    ) -> Self {
        let question = question.into();
        let (confirm_name, confirm_label) = (confirm.0.as_ref().to_owned(), confirm.1.into());
        let (cancel_name, cancel_label) = (cancel.0.as_ref().to_owned(), cancel.1.into());
        self.modal(title, move |builder| {
            builder
                .text(question)
                .primary_button(confirm_name, confirm_label)
                .button(cancel_name, cancel_label)
        })
    }

    /// A labelled group of rows, kept together on the page.
    ///
    /// The commonest shape in the whole example set and the one nine
    /// applications each wrote out longhand: a heading that names what follows,
    /// optionally a count beside it, and then the rows. Written as one call so
    /// the header and its rows are always the same distance apart, and so the
    /// paginator is given them as one thing to place rather than two it may
    /// separate.
    #[must_use]
    pub fn section_rows<I, N, T, S, L>(
        self,
        title: impl Into<String>,
        value: Option<String>,
        rows: I,
    ) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
    {
        let builder = match value {
            Some(value) => self.section_with_value(title, value),
            None => self.section(title),
        };
        builder.rows(rows)
    }

    /// Adds a consistent empty, offline, denied, or error presentation.
    ///
    /// Chain [`Self::button`] when the condition has a recovery action. The
    /// state itself owns no action so an empty collection is never forced to
    /// pretend it can be fixed.
    ///
    /// Set as a splash rather than a heading and a paragraph, because a
    /// heading and a paragraph flow from the top and leave a thousand pixels
    /// of white beneath them: correct for reading, wrong for a page with six
    /// words on it. The splash centres itself in the room that is left after
    /// whatever is chained on, so a recovery button still lands under it.
    ///
    /// No banner. This used to raise one as well, which put two reports of one
    /// event on the same empty page, the banner being the vaguer of the two:
    /// "Access is not available" in a grey strip above "Permission needed" set
    /// large in the middle. A banner is for a failure that has to sit over
    /// content the reader is already looking at, and this is the case where
    /// there is none.
    #[must_use]
    pub fn standard_state(self, state: StandardState, message: impl Into<String>) -> Self {
        self.splash(Some(state.glyph()), state.title(), message)
    }

    #[must_use]
    pub fn empty_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::Empty, message)
    }

    #[must_use]
    pub fn offline_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::Offline, message)
    }

    #[must_use]
    pub fn permission_denied_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::PermissionDenied, message)
    }

    #[must_use]
    pub fn error_state(self, message: impl Into<String>) -> Self {
        self.standard_state(StandardState::Error, message)
    }

    /// A failed task's whole-screen presentation, with the way out of it.
    ///
    /// Chain this instead of writing [`Self::standard_state`] and a recovery
    /// button by hand, so that every application recovers from a failure the
    /// same way and gains a new route the day the SDK does.
    ///
    /// Being offline is the one failure the reader can fix on the device, and
    /// the only one that gets a second control: [`JOIN_WIFI`], which
    /// [`AppRunner`] answers by opening Settings on the Wi-Fi screen. The two
    /// controls sit side by side because they are alternatives, and joining is
    /// the primary of the pair because retrying a request on a reader with no
    /// network will fail the same way it just did.
    ///
    /// A failure that is not retryable gets no control at all, rather than a
    /// Try again that is known in advance to fail.
    #[must_use]
    pub fn failure_state(self, failure: Failure, retry: impl AsRef<str>) -> Self {
        let screen = self.standard_state(failure.state, failure.advice);
        match (failure.state, failure.retryable) {
            (StandardState::Offline, _) => {
                screen.buttons([(JOIN_WIFI, "Join Wi-Fi"), (retry.as_ref(), "Try again")])
            }
            (_, true) => screen.primary_button(retry, "Try again"),
            (_, false) => screen,
        }
    }

    /// Builds a sparse, full-screen confirmation using standard controls.
    ///
    /// Kobo applications do not open floating windows: the display is a
    /// single retained page, so confirmations replace the page and use the
    /// application's typed navigator to return.
    #[must_use]
    pub fn confirmation(
        self,
        title: impl Into<String>,
        message: impl Into<String>,
        primary: DialogAction,
        secondary: DialogAction,
    ) -> Self {
        let DialogAction {
            name: primary_name,
            label: primary_label,
            state: primary_state,
        } = primary;
        let DialogAction {
            name: secondary_name,
            label: secondary_label,
            state: secondary_state,
        } = secondary;
        self.heading(title)
            .text(message)
            .divider()
            .button_with_state(primary_name, primary_label, primary_state)
            .button_with_state(secondary_name, secondary_label, secondary_state)
    }

    /// A paragraph set in from the left by `depth` levels, with a rule beside
    /// it, for a reply that answers what came before it.
    ///
    /// Depth is clamped to [`MAX_QUOTE_DEPTH`], so a thread that nests forty
    /// deep still reads: the deepest replies share an indent and say how deep
    /// they really are in their own words.
    #[must_use]
    pub fn quote(self, depth: u8, text: impl Into<String>) -> Self {
        self.quote_as(depth, QuoteRole::Body, text)
    }

    /// The line above a reply that says who wrote it and when.
    ///
    /// Set as metadata rather than as prose: a byline drawn at body size in
    /// body ink reads as the comment's opening sentence, which is what a real
    /// thread on a real panel looked like before this existed.
    #[must_use]
    pub fn byline(self, depth: u8, text: impl Into<String>) -> Self {
        self.quote_as(depth, QuoteRole::Byline, text)
    }

    /// A byline that folds away everything underneath it.
    ///
    /// `name` is the action sent when it is tapped; `hidden` is how many
    /// replies are behind it, which is drawn only while it is shut. What
    /// folding *does* is the application's business -- the renderer only sends
    /// the tap -- because only the application knows where the subtree ends.
    #[must_use]
    pub fn folding_byline(
        self,
        depth: u8,
        text: impl Into<String>,
        name: impl AsRef<str>,
        collapsed: bool,
        hidden: u16,
    ) -> Self {
        let action = action_id(name.as_ref());
        self.quote_full(
            depth,
            QuoteRole::Byline,
            text,
            Some(Fold {
                action,
                collapsed,
                hidden,
            }),
        )
    }

    /// A paragraph of a thread, saying what it is for.
    #[must_use]
    pub fn quote_as(self, depth: u8, role: QuoteRole, text: impl Into<String>) -> Self {
        self.quote_full(depth, role, text, None)
    }

    fn quote_full(
        mut self,
        depth: u8,
        role: QuoteRole,
        text: impl Into<String>,
        fold: Option<Fold>,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Quote {
            id,
            depth: depth.min(MAX_QUOTE_DEPTH),
            role,
            text: text.into(),
            fold,
        });
        self
    }

    #[must_use]
    pub fn button(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        self.button_with_state(name, label, ControlState::Enabled)
    }

    /// Adds the one control the screen exists for, drawn filled.
    ///
    /// At most one per screen. A fill is the loudest mark this panel can make
    /// and the slowest to clear, so spending it on every control (which is
    /// what the platform used to do) leaves the reader with nothing to aim at
    /// and the panel with a slab to erase.
    #[must_use]
    pub fn primary_button(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        let id = self.next_id();
        self.nodes.push(Node::Button {
            id,
            action,
            label: label.into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Primary,
        });
        self
    }

    /// Adds a button that is visible but cannot currently be activated.
    #[must_use]
    pub fn disabled_button(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        self.button_with_state(name, label, ControlState::Disabled)
    }

    /// Puts two or three secondary actions on one line.
    ///
    /// Stacked, each of them takes the full width of the panel to say one
    /// word, and a screen that ends in three of those reads as a form rather
    /// than as a page with some things you can do to it. Side by side they are
    /// as wide as they need to be and the reader can see at a glance that they
    /// belong together. Both platforms do this: a `UIStackView` of secondary
    /// buttons on iOS, a `Row` of `OutlinedButton`s on Android.
    ///
    /// This is [`ScreenBuilder::band`] with a slot per action, not a new kind
    /// of thing, so a narrow panel still stacks them by itself rather than
    /// squeezing three words into a third of a screen each.
    ///
    /// Anything past the third is dropped, which is what a band does anyway. A
    /// row of four controls is a menu, and the overflow menu is what that is
    /// for.
    #[must_use]
    pub fn buttons<I, N, L>(self, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let mut actions = actions
            .into_iter()
            .take(MAX_BAND_SLOTS)
            .map(|(name, label)| (name.as_ref().to_owned(), label.into()));
        match (actions.next(), actions.next()) {
            (None, _) => self,
            // One action side by side with nothing is a button, and going
            // through a band would only cost a node and read the same.
            (Some((name, label)), None) => self.button(name, label),
            (Some(first), Some(second)) => self.band(
                BandAlign::Middle,
                [first, second].into_iter().chain(actions).map(
                    |(name, label): (String, String)| {
                        (SlotWidth::Fill, move |slot: Self| slot.button(name, label))
                    },
                ),
            ),
        }
    }

    /// Adds a button with explicit semantic enabled state.
    #[must_use]
    pub fn button_with_state(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        state: ControlState,
    ) -> Self {
        let action = self.register(name.as_ref());
        let id = self.next_id();
        self.nodes.push(Node::Button {
            id,
            action,
            label: label.into(),
            state,
            emphasis: Emphasis::Normal,
        });
        self
    }

    #[must_use]
    pub fn divider(mut self) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Divider { id });
        self
    }

    /// Adds vertical space from the design scale.
    ///
    /// There is deliberately no pixel argument. Authors choose an intent and
    /// the renderer decides what that measures on the panel in front of it.
    #[must_use]
    pub fn spacer(mut self, space: Space) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Spacer { id, space });
        self
    }

    /// Pushes everything after it to the foot of the panel.
    ///
    /// The keyboard is what this is for. It is the tallest thing a screen
    /// draws and it belongs under the thumbs, but it is placed in flow like
    /// every other node, so a compose screen with a prompt and a line of typed
    /// text put the keys across the middle of the panel with a third of a page
    /// of paper underneath them.
    ///
    /// It only ever pushes down. A screen that is already full is laid out
    /// exactly as it was.
    #[must_use]
    pub fn fill(mut self) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Flex { id });
        self
    }

    /// Adds a progress bar. Values above a hundred are clamped rather than
    /// rejected, because that is a caller mistake and not a reason to fail.
    #[must_use]
    pub fn progress(mut self, value: u8) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Progress {
            id,
            value: Percent::new(value),
        });
        self
    }

    #[must_use]
    pub fn paged_list<I, S>(mut self, page: u16, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let id = self.next_id();
        self.nodes.push(Node::PagedList {
            id,
            page,
            items: items.into_iter().map(Into::into).collect(),
        });
        self
    }

    #[must_use]
    pub fn action(&self, name: &str) -> Option<ActionId> {
        self.actions
            .iter()
            .find_map(|(known, id)| (known == name).then_some(*id))
    }

    /// Asks for the reader's Back to arrive as an action first.
    ///
    /// The Back control belongs to the runtime and always leads out of the
    /// application in the end; this only asks for first refusal, so a screen
    /// reached from inside the application can return to where it was reached
    /// from instead of dropping the reader at the launcher. Pass
    /// [`Navigator::can_go_back`] and the behaviour follows the back stack for
    /// free: deep screens pop, the root leaves.
    ///
    /// The offer expires. An application that sets this and then draws nothing
    /// in answer to [`ActionId::BACK`] is left behind and the launcher appears
    /// anyway, which is why setting it can never strand a reader.
    #[must_use]
    pub const fn owns_back(mut self, owns_back: bool) -> Self {
        self.owns_back = owns_back;
        self
    }

    /// Asks for a text size other than the reader's own.
    ///
    /// Almost no screen should. The scale is an accessibility preference and
    /// overriding it overrules someone who has already said how large they
    /// need type to be. The case it exists for is a reader, where the size of
    /// the body text is the thing being adjusted and the adjustment belongs to
    /// the book.
    ///
    /// Paginate with [`Context::metrics_at`] using the same scale. Measuring at
    /// one size and drawing at another loses the end of every page.
    #[must_use]
    pub const fn text_scale(mut self, scale: kobo_ui::TextScale) -> Self {
        self.text_scale = Some(scale);
        self
    }

    /// Says this screen's text is a book rather than an interface.
    ///
    /// Sets prose in a serif drawn for continuous reading (the device's own
    /// reading face where it has one) and opens the lines to the measure books
    /// have always used. For the pages of a reader and nothing else: the
    /// interface face is chosen so a label glanced at once cannot be misread,
    /// which is a different problem with a different answer.
    ///
    /// Paginate with [`Context::paginate_reading`], because a serif sets the
    /// same words wider and a page measured in the wrong face loses its last
    /// lines.
    #[must_use]
    pub const fn reading(mut self, reading: bool) -> Self {
        self.reading = reading;
        self
    }

    /// Uses a publisher font previously handed to the runtime for book prose.
    #[must_use]
    pub const fn reading_font(mut self, font: FontHandle) -> Self {
        self.reading_font = Some(font);
        self
    }

    /// Hangs a popover off the control named `anchor`.
    ///
    /// The closure builds the overlay's contents with the same builder the
    /// screen uses, so ids and action names carry on from where the screen
    /// left off: a control inside a popover is named and read exactly like a
    /// control on the screen, and nothing has to be told which is which.
    #[must_use]
    pub fn popover(self, anchor: impl AsRef<str>, build: impl FnOnce(Self) -> Self) -> Self {
        let anchor = action_id(anchor.as_ref());
        self.overlay_with(OverlayKind::Popover { anchor }, String::new(), build)
    }

    /// Puts a question over the screen that has to be answered.
    #[must_use]
    pub fn modal(self, title: impl Into<String>, build: impl FnOnce(Self) -> Self) -> Self {
        self.overlay_with(OverlayKind::Modal, title.into(), build)
    }

    fn overlay_with(
        mut self,
        kind: OverlayKind,
        title: String,
        build: impl FnOnce(Self) -> Self,
    ) -> Self {
        // The screen's nodes are set aside so the closure builds into an empty
        // list, then put back. Threading the builder through rather than
        // handing the closure a fresh one is what keeps one id counter and one
        // action table for the whole screen.
        let outer = std::mem::take(&mut self.nodes);
        let id = self.next_id();
        let mut done = build(self);
        let nodes = std::mem::replace(&mut done.nodes, outer);
        done.overlay = Some(Box::new(Overlay {
            id,
            kind,
            title,
            nodes,
        }));
        done
    }

    /// Adds the fixed top bar.
    ///
    /// Calling this twice replaces the bar rather than adding a second one. A
    /// screen has at most one, which is a property of the type rather than a
    /// rule the author has to follow.
    #[must_use]
    pub fn top_bar(mut self, title: impl Into<String>) -> Self {
        let id = self.next_id();
        self.top_bar = Some(TopBar::new(id, title));
        self
    }

    /// Adds an action to the top bar, right to left.
    ///
    /// At most two; see `kobo_ui::MAX_BAR_ACTIONS`. A no-op if there is no top
    /// bar, because an action with nowhere to live is an author mistake that
    /// should not silently become a floating button.
    #[must_use]
    pub fn top_bar_action(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(top_bar) = self.top_bar.take() {
            self.top_bar = Some(top_bar.with_action(BarAction::new(action, label)));
        }
        self
    }

    /// The same, drawn as one of the built-in icons.
    ///
    /// For a control whose meaning has a picture everyone already knows: the
    /// front light, a search. The label is still required, because it is what
    /// the control is called everywhere that is not the panel -- a preview, a
    /// test, a log -- and a mark with no word anywhere near it is a puzzle.
    #[must_use]
    pub fn top_bar_glyph(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        glyph: kobo_ui::Glyph,
    ) -> Self {
        let action = self.register(name.as_ref());
        if let Some(top_bar) = self.top_bar.take() {
            self.top_bar =
                Some(top_bar.with_action(BarAction::new(action, label).with_glyph(glyph)));
        }
        self
    }

    /// Puts the rest of this screen's verbs under three dots in the top bar.
    ///
    /// The answer to a top bar with more than [`kobo_ui::MAX_BAR_ACTIONS`]
    /// things to offer: the bar does not grow, the third verb goes under the
    /// dots. Nine applications were each about to rebuild this out of
    /// `top_bar_glyph` plus `popover` plus a column of buttons, and they would
    /// not have agreed on the glyph, the order or the dismissal.
    ///
    /// `open` is the application's, the way `expanded` is the application's in
    /// Compose's `DropdownMenu`: whether the menu is showing is a fact about
    /// the screen, and a screen is drawn from state here rather than mutated.
    /// The dots are drawn either way, so the bar does not jump when the menu
    /// opens.
    ///
    /// Closing it is not the application's. The popover draws a caret pointing
    /// at the control it came out of, and a tap anywhere outside it arrives as
    /// `ActionId::BACK`, because the scrim a popover puts down reports a miss.
    /// All the application does with that is set `open` back to false.
    ///
    /// A no-op with no items, rather than three dots that open onto nothing.
    #[must_use]
    pub fn top_bar_overflow<I, N, L>(self, name: impl AsRef<str>, open: bool, items: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let items = items
            .into_iter()
            .map(|(name, label)| (name.as_ref().to_owned(), label.into()))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return self;
        }
        let name = name.as_ref().to_owned();
        let screen = self.top_bar_glyph(&name, "More", kobo_ui::Glyph::More);
        if !open {
            return screen;
        }
        screen.popover(&name, move |builder| {
            items.into_iter().fold(builder, |builder, (name, label)| {
                builder.button(name, label)
            })
        })
    }

    /// The menu behind a row's overflow mark.
    ///
    /// The companion to [`Self::rows_with_menu`], and the same shape as
    /// [`Self::top_bar_overflow`]: pass the mark's name, whether it is open,
    /// and what it offers. `open` is a property of the application's state
    /// rather than something this remembers, for the reason every overlay in
    /// this SDK works that way -- the tap that closes a popover arrives as
    /// `ActionId::BACK` from the scrim, and an application that has to notice
    /// that tap itself is an application that sometimes forgets.
    ///
    /// One caution the bar's version does not need: pass `open` as false when
    /// the row is not on the current page. A popover anchored to a control
    /// that is not drawn has nothing to point at.
    /// Each item is a name, a word and a mark, and is drawn as a row rather
    /// than a button. A menu is a list of things to do to one entry, and a
    /// stack of full-width outlined buttons reads as a form; a row also gives
    /// the mark somewhere to stand, which is what lets a destructive item say
    /// "Delete" beside a bin instead of spelling the whole verb out.
    #[must_use]
    pub fn row_overflow<I, N, L>(self, anchor: impl AsRef<str>, open: bool, items: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        if !open {
            return self;
        }
        let items = items
            .into_iter()
            .map(|(name, label, glyph)| (name.as_ref().to_owned(), label.into(), glyph))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return self;
        }
        self.popover(anchor.as_ref(), move |builder| {
            builder.rows(
                items
                    .into_iter()
                    .map(|(name, label, glyph)| (name, label, String::new(), glyph)),
            )
        })
    }

    /// Adds the fixed bottom bar.
    ///
    /// Note there is no back destination to add: back belongs to the runtime's
    /// navigation stack, so it appears automatically wherever there is
    /// somewhere to go back to and cannot be omitted by an application.
    /// Turns the sides of the content area into page turns.
    ///
    /// This is how every Kobo has worked since the first one: tap the left of
    /// the page to go back, anywhere else to go on. Actions are named, like
    /// every other action, so the same two intents can later be raised by the
    /// physical page buttons some models have.
    ///
    /// Controls always win. A tap that lands on a button, a row or a keyboard
    /// key is that control's; the zones only ever collect taps that would
    /// otherwise have done nothing.
    #[must_use]
    pub fn page_turns(mut self, previous: impl AsRef<str>, next: impl AsRef<str>) -> Self {
        let previous = self.register(previous.as_ref());
        let next = self.register(next.as_ref());
        self.page_turns = Some(kobo_ui::PageTurns::new(previous, next));
        self
    }

    /// Says which page of how many the turns are moving through.
    ///
    /// `page` is one-based. Drawn centred at the foot of the content, muted,
    /// costing one caption line. Without it a paginated list gives the reader
    /// no way to tell a page turn from a list that did not move -- the
    /// catalogue cut its shelf into as many as fifty-four pages and said
    /// nothing about which one was showing.
    ///
    /// Has no effect unless [`Self::page_turns`] was asked for as well.
    #[must_use]
    pub fn page_position(mut self, page: u16, of: u16) -> Self {
        self.page_turns = self.page_turns.map(|turns| turns.with_position(page, of));
        self
    }

    /// Draws Folio's passive right-margin page rail.
    ///
    /// `page` is zero-based, matching application pagination vectors. The
    /// rail is display-only, so it is never a slider or a competing gesture.
    #[must_use]
    pub fn page_rail(mut self, page: u16, of: u16) -> Self {
        if of > 1 {
            let id = self.next_id();
            self.nodes.push(Node::PageRail { id, page, of });
        }
        self
    }

    /// Adds a middle column that asks for this screen's own controls.
    ///
    /// For a screen that carries nothing at the foot, which is every reading
    /// screen: without this there is no way to reach a setting with a finger,
    /// because the whole content area is spoken for by page turns. Left third
    /// back, middle third the controls, right third forward, which is what
    /// every other reader on this hardware does.
    ///
    /// Has no effect unless [`Self::page_turns`] was asked for as well, since
    /// the zones are one arrangement rather than three separate ones.
    #[must_use]
    pub fn reading_menu(mut self, menu: impl AsRef<str>) -> Self {
        let menu = self.register(menu.as_ref());
        self.page_turns = self.page_turns.map(|turns| turns.with_menu(menu));
        self
    }

    /// Sends an optional secondary `action` when a finger is held still on
    /// empty content.
    ///
    /// A hold is an accelerator, never the only way to reach navigation,
    /// accessibility, confirmation, destructive, or primary behavior. Keep a
    /// visible control or overflow entry for anything a reader must discover.
    /// Ordinary control taps remain immediate; holding a real control still
    /// activates that control rather than hiding it behind a gesture.
    #[must_use]
    pub fn hold(mut self, action: impl AsRef<str>) -> Self {
        self.hold = Some(self.register(action.as_ref()));
        self
    }

    /// Adds the fixed bar at the bottom of the screen.
    ///
    /// `selected` takes an index or `None`. `None` is for a bar whose entries
    /// are actions rather than places (page back, page forward, the way out)
    /// where marking any of them as current would tell the reader they are
    /// somewhere they are not.
    #[must_use]
    pub fn nav_bar<I, N, L, S>(mut self, selected: S, destinations: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
        S: Into<Option<usize>>,
    {
        let id = self.next_id();
        let destinations = destinations
            .into_iter()
            .map(|(name, label)| BarAction::new(self.register(name.as_ref()), label))
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::new(id, destinations, selected.into()));
        self.bottom_action = None;
        self
    }

    /// Adds a destination bar whose labels keep their recognisable glyphs.
    #[must_use]
    pub fn nav_bar_marked<I, N, L, S>(mut self, selected: S, destinations: I) -> Self
    where
        I: IntoIterator<Item = (N, L, kobo_ui::Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
        S: Into<Option<usize>>,
    {
        let id = self.next_id();
        let destinations = destinations
            .into_iter()
            .map(|(name, label, glyph)| {
                BarAction::new(self.register(name.as_ref()), label).with_glyph(glyph)
            })
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::new(id, destinations, selected.into()));
        self.bottom_action = None;
        self
    }

    /// Pins the verbs belonging to this screen to the bottom band.
    ///
    /// The other half of [`Self::nav_bar`], and the reason that one should now
    /// always be given a selection. A nav bar names places, is the same on
    /// every screen of an application, and marks the one you are on. An action
    /// bar names things to do here, is free to change from screen to screen,
    /// and marks nothing -- because none of its entries is a place you could
    /// be standing.
    ///
    /// Android draws exactly this line between `NavigationBar` and
    /// `BottomAppBar`; iOS between a tab bar and a toolbar. Before this, three
    /// screens in the example set passed `None` to `nav_bar` to get a bar of
    /// verbs, which worked but meant nothing could tell a bar that had
    /// forgotten to say where the reader was from one that had nowhere to say.
    ///
    /// Two or three actions. A third is dropped on a panel too narrow to give
    /// all of them a finger's width, and anything past three belongs in an
    /// overflow menu.
    ///
    /// Mutually exclusive with [`Self::nav_bar`] and [`Self::bottom_action`]:
    /// they are all the same band.
    #[must_use]
    pub fn action_bar<I, N, L>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let actions = actions
            .into_iter()
            .map(|(name, label)| BarAction::new(self.register(name.as_ref()), label))
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::actions(id, actions));
        self.bottom_action = None;
        self
    }

    /// The same, with a mark on each entry that has one.
    ///
    /// A bar slot is a third of a panel wide and a bar entry is a verb, so the
    /// ones with a picture everyone already knows should show it: a chevron
    /// for a page turn, a house for the way out. The mark is drawn above the
    /// word rather than instead of it, because this band is frequently the
    /// only way off a screen and is the last place to make somebody guess.
    #[must_use]
    pub fn action_bar_marked<I, N, L>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Option<kobo_ui::Glyph>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let actions = actions
            .into_iter()
            .map(|(name, label, glyph)| {
                let action = BarAction::new(self.register(name.as_ref()), label);
                match glyph {
                    Some(glyph) => action.with_glyph(glyph),
                    None => action,
                }
            })
            .collect::<Vec<_>>();
        self.warn_second_bottom_bar(id);
        self.nav_bar = Some(NavBar::actions(id, actions));
        self.bottom_action = None;
        self
    }

    /// Pins one control to the bottom of the panel, where a bar would go.
    ///
    /// For a screen with a single way off it. Prefer this to a button at the
    /// end of the flow whenever the control must always be reachable: layout
    /// reserves this band before it places any content, so nothing above can
    /// push the control off the panel, and a page that runs long loses its
    /// last line rather than the only way out. A trailing button reserves
    /// nothing, and the launcher shipped with its way back to the Kobo reader
    /// hanging over the bottom edge of the screen because of it.
    ///
    /// Mutually exclusive with [`Self::nav_bar`], they are the same band.
    #[must_use]
    pub fn bottom_action(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let id = self.next_id();
        let action = BarAction::new(self.register(name.as_ref()), label);
        self.warn_second_bottom_bar(id);
        self.bottom_action = Some(BottomAction::new(id, action));
        self.nav_bar = None;
        self
    }

    /// The same, with a mark beside the word.
    ///
    /// The mark sits next to the label rather than replacing it: one pinned
    /// control has the width for both, and this is the band a reader uses to
    /// leave.
    #[must_use]
    pub fn bottom_action_marked(
        mut self,
        name: impl AsRef<str>,
        label: impl Into<String>,
        glyph: kobo_ui::Glyph,
    ) -> Self {
        let id = self.next_id();
        let action = BarAction::new(self.register(name.as_ref()), label).with_glyph(glyph);
        self.warn_second_bottom_bar(id);
        self.bottom_action = Some(BottomAction::new(id, action));
        self.nav_bar = None;
        self
    }

    /// Adds a grid of tiles. Columns are chosen from the panel's physical
    /// width, so the author never picks a count that is wrong on some device.
    #[must_use]
    pub fn tiles<I, N, L>(mut self, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph)| Tile::new(self.register(name.as_ref()), label, glyph))
            .collect();
        self.nodes.push(Node::TileGrid {
            id,
            tiles,
            shape: TileShape::Square,
        });
        self
    }

    /// Adds a text field showing what is currently in it.
    ///
    /// Tapping yields `name`; route that to your own keyboard screen. The
    /// field does not summon a keyboard, because the runtime does not own one.
    /// What it does is show the query, which is the part a button could not do
    /// and the reason a search entry point used to be an unlabelled ellipsis
    /// in the top bar.
    #[must_use]
    pub fn field(
        mut self,
        name: impl AsRef<str>,
        value: impl Into<String>,
        placeholder: impl Into<String>,
    ) -> Self {
        let id = self.next_id();
        let action = self.register(name.as_ref());
        self.nodes.push(Node::Field {
            id,
            action,
            value: value.into(),
            placeholder: placeholder.into(),
            clear: None,
        });
        self
    }

    /// Puts a cross in the field just added, to empty it.
    ///
    /// Does nothing if the last node is not a field, and nothing if that field
    /// is already empty: a cross beside an empty box is a control that cannot
    /// do anything, and one of those on every search screen teaches readers
    /// that controls on this platform are decorative.
    #[must_use]
    pub fn field_clear(mut self, name: impl AsRef<str>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Field { value, clear, .. }) = self.nodes.last_mut() {
            if !value.is_empty() {
                *clear = Some(action);
            }
        }
        self
    }

    /// Adds a wrapping run of short tappable labels.
    ///
    /// Subjects, facets, languages, recent searches. The renderer wraps them;
    /// you supply no geometry. Entries past [`MAX_CHIPS`] are dropped and
    /// reported by `validate`.
    #[must_use]
    pub fn chips<I, N, L>(mut self, chips: I) -> Self
    where
        I: IntoIterator<Item = (N, L, bool)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let chips = chips
            .into_iter()
            .map(|(name, label, selected)| {
                Chip::new(self.register(name.as_ref()), label).selected(selected)
            })
            .collect();
        self.nodes.push(Node::Chips { id, chips });
        self
    }

    /// Adds up to [`MAX_TABS`] peer views of the current screen.
    ///
    /// For filters on one destination. Destinations go in [`Self::nav_bar`],
    /// which is pinned to the bottom and says "you have gone somewhere else";
    /// a tab says "you are still here, looking at it differently".
    #[must_use]
    pub fn tabs<I, N, L>(mut self, selected: usize, tabs: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tabs = tabs
            .into_iter()
            .map(|(name, label)| Chip::new(self.register(name.as_ref()), label))
            .collect();
        self.nodes.push(Node::Tabs { id, tabs, selected });
        self
    }

    /// Adds a grid of tiles, each one configured by a closure.
    ///
    /// This is the general form, and the reason there will not be a fifth
    /// `*_tiles` method. [`Self::tiles`] and [`Self::picture_tiles`] each fixed
    /// one combination of a tile's optional parts into a tuple, so every part
    /// added afterwards would have needed a new method and a new arity. Here
    /// the tile arrives already registered and the closure says what else is
    /// true of it, exactly as a Compose slot or a `SwiftUI` modifier chain does:
    ///
    /// ```ignore
    /// screen.tile_grid(TileShape::Portrait, [
    ///     ("bleak-house", "Bleak House", Glyph::Book, |tile: Tile| {
    ///         tile.with_subtitle("Charles Dickens")
    ///             .with_state(TileState::Held)
    ///     }),
    /// ])
    /// ```
    ///
    /// A tile marked [`TileState::Unavailable`] keeps its place in the grid and
    /// stops answering taps, which is the whole point: a shelf with a gap in it
    /// is a shelf that has lost its alignment.
    #[must_use]
    pub fn tile_grid<I, N, L, F>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, F)>,
        N: AsRef<str>,
        L: Into<String>,
        F: FnOnce(Tile) -> Tile,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, configure)| {
                configure(Tile::new(self.register(name.as_ref()), label, glyph))
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Adds tiles with an optional hold accelerator for a secondary menu.
    ///
    /// `menu` must also be exposed by a visible overflow, details, or section
    /// control. Holding is a convenience for experienced readers, never the
    /// only route to an action.
    #[must_use]
    pub fn contextual_tiles<I, N, L, M>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, M)>,
        N: AsRef<str>,
        L: Into<String>,
        M: AsRef<str>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, menu)| {
                let action = self.register(name.as_ref());
                let menu = self.register(menu.as_ref());
                Tile::new(action, label, glyph).with_menu(menu)
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Adds launcher tiles directly from application metadata.
    #[must_use]
    pub fn apps<I>(mut self, apps: I) -> Self
    where
        I: IntoIterator<Item = AppMetadata>,
    {
        let id = self.next_id();
        let tiles = apps
            .into_iter()
            .map(|app| app.tile(self.register(app.id)))
            .collect();
        self.nodes.push(Node::TileGrid {
            id,
            tiles,
            shape: TileShape::Square,
        });
        self
    }

    /// Adds a grid of tiles that may each carry a picture.
    ///
    /// Use [`TileShape::Portrait`] for covers and posters: a square cell
    /// letterboxes a book cover into roughly half its own area, which is what
    /// makes a shelf of covers look like a grid of stamps.
    ///
    /// A tile whose picture the runtime does not have falls back to its glyph,
    /// so a shelf is usable while the covers are still arriving.
    #[must_use]
    pub fn picture_tiles<I, N, L>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, Option<TilePicture>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, picture)| {
                let tile = Tile::new(self.register(name.as_ref()), label, glyph);
                match picture {
                    Some(picture) => tile.with_picture(picture),
                    None => tile,
                }
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Shows one picture, as large as the width and `max_height_mm` allow.
    ///
    /// The height is a physical measurement rather than a pixel count so that
    /// the same screen gives a picture the same share of the panel on a Clara
    /// and on an Elipsa.
    #[must_use]
    pub fn picture(self, picture: TilePicture, max_height_mm: u16) -> Self {
        self.drawn_picture(picture, max_height_mm, true)
    }

    /// The same, without a rule around it.
    ///
    /// For a picture that is part of the text rather than an illustration of
    /// it -- a formula set on its own line, say. An edge tells a reader where
    /// an illustration stops; drawn around a line of mathematics it only says
    /// that the line was drawn rather than written, which is not something the
    /// reader needs to know.
    #[must_use]
    pub fn unframed_picture(self, picture: TilePicture, max_height_mm: u16) -> Self {
        self.drawn_picture(picture, max_height_mm, false)
    }

    #[must_use]
    fn drawn_picture(mut self, picture: TilePicture, max_height_mm: u16, framed: bool) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Picture {
            id,
            handle: picture.handle,
            source: picture.source,
            max_height_tenths_mm: max_height_mm.saturating_mul(10),
            framed,
        });
        self
    }

    /// Lists entries that each need a sentence of explanation.
    ///
    /// Prefer this over [`Self::tiles`] whenever a one-word label would not be
    /// enough. A tile is square and spends most of its area on nothing, so a
    /// screen of tiles holds very few entries; a row holds a title, a summary
    /// and a glyph in a single finger-height band.
    #[must_use]
    pub fn rows<I, N, T, S, L>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, lead) in source.by_ref().take(MAX_ROWS) {
            rows.push(Row::new(self.register(name.as_ref()), title, summary, lead));
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// The same, with an overflow mark against the right edge of each row.
    ///
    /// The mark is a vertical three dot control naming an action of its own,
    /// so a tap on it is not a tap on the row. Use it for the things a reader
    /// might want to do *to* an entry rather than *with* it: stop following a
    /// feed, forget a book, remove a key. What the action opens is the
    /// application's business, and a popover is usually the right answer.
    ///
    /// An empty menu name means no mark on that row, exactly as an empty
    /// trailing value means no value.
    #[must_use]
    pub fn rows_with_menu<I, N, T, S, L, M>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L, M)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
        M: AsRef<str>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, lead, menu) in source.by_ref().take(MAX_ROWS) {
            let row = Row::new(self.register(name.as_ref()), title, summary, lead);
            let menu = menu.as_ref();
            rows.push(if menu.is_empty() {
                row
            } else {
                let action = self.register(menu);
                row.with_menu(action)
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// The same, with a short value against the right edge of each row.
    ///
    /// A score, a size, a date, a count. A separate method rather than a fifth
    /// element on [`Self::rows`] because most lists have no such value, and a
    /// tuple whose last member is almost always empty is a tuple every caller
    /// has to read twice.
    ///
    /// An empty value means no value, exactly as an empty summary does. The
    /// value is measured before the title is wrapped, so a long title gives up
    /// its own room rather than pushing the value off the panel.
    #[must_use]
    pub fn rows_with_trailing<I, N, T, S, L, V>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L, V)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
        V: Into<String>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, lead, trailing) in source.by_ref().take(MAX_ROWS) {
            let row = Row::new(self.register(name.as_ref()), title, summary, lead);
            let trailing = trailing.into();
            rows.push(if trailing.is_empty() {
                row
            } else {
                row.with_trailing(trailing)
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// A list of things to be done, some of which are.
    ///
    /// The same rows, with the state carried rather than drawn: an application
    /// says whether each entry is finished and the renderer decides what
    /// finished looks like. That is why there is no way to ask for a line
    /// through a piece of text anywhere else in this SDK.
    ///
    /// Tapping a row is what completes it, and only the row that changed is
    /// repainted, so ticking something off costs one fast partial refresh
    /// rather than a whole screen.
    #[must_use]
    pub fn checklist<I, N, T, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, bool)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
    {
        let id = self.next_id();
        let mut source = items.into_iter();
        let mut rows = Vec::new();
        for (name, title, summary, done) in source.by_ref().take(MAX_ROWS) {
            let glyph = if done { Glyph::Check } else { Glyph::Circle };
            rows.push(Row::new(self.register(name.as_ref()), title, summary, glyph).done(done));
        }
        if source.next().is_some() {
            self.warn_limit(id, "rows", MAX_ROWS);
        }
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// A grid of characters, for output that was written to be read in columns.
    ///
    /// Everything else in this builder takes meaning and lets the runtime
    /// decide on appearance. This takes rows that are already positioned,
    /// because in a character grid the position *is* the meaning: a table, a
    /// diff or a shell prompt stops saying what it said the moment something
    /// re-wraps it.
    ///
    /// The grid is not negotiable from here. Ask [`kobo_ui::terminal_grid_for`]
    /// what size the rows should be before filling them, so that whatever is
    /// producing the text is told the same width the panel will show.
    #[must_use]
    pub fn terminal<I, R>(mut self, rows: I, cursor: Option<Caret>) -> Self
    where
        I: IntoIterator<Item = R>,
        R: Into<String>,
    {
        let id = self.next_id();
        let mut source = rows.into_iter();
        let rows = source
            .by_ref()
            .take(MAX_TERMINAL_ROWS)
            .map(Into::into)
            .collect();
        if source.next().is_some() {
            self.warn_limit(id, "terminal rows", MAX_TERMINAL_ROWS);
        }
        self.nodes.push(Node::Terminal { id, rows, cursor });
        self
    }
    /// A grid of buttons.
    ///
    /// The general one: the caller picks the columns, so a board, a keypad and
    /// an on-screen keyboard are all this, rather than three primitives that
    /// each have to be added to the layout engine, the renderer, the hit test
    /// and the wire format before anybody can use them.
    ///
    /// `square` gives cells as tall as they are wide, which is what makes a
    /// board look like a board. Without it a cell is one touch target high,
    /// which is what a keyboard wants.
    #[must_use]
    pub fn grid<I, N, L>(mut self, columns: u8, square: bool, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label) in source.by_ref().take(MAX_CELLS) {
            cells.push(Cell::new(self.register(name.as_ref()), label));
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square,
            cells,
        });
        self
    }

    /// A square grid whose filled cells are drawn as marks rather than words.
    ///
    /// For a board. A letter set at label size in a cell a finger and a half
    /// wide is a caption in the middle of an empty square, which is what a
    /// tic-tac-toe board looked like: the "O" was smaller than the heading
    /// above it. A mark is drawn at three fifths of the cell, so the board
    /// reads as a board from arm's length.
    ///
    /// `None` leaves the cell empty and still tappable, because an unplayed
    /// square is the one a reader is aiming at.
    #[must_use]
    pub fn board<I, N, L>(mut self, columns: u8, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Option<Glyph>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, glyph) in source.by_ref().take(MAX_CELLS) {
            let cell = Cell::new(self.register(name.as_ref()), label);
            cells.push(match glyph {
                Some(glyph) => cell.with_glyph(glyph),
                None => cell,
            });
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square: true,
            cells,
        });
        self
    }

    /// A row of buttons that each have a picture as well as a word.
    ///
    /// For the handful of actions that have a drawing everybody already knows:
    /// the transport controls, chiefly. Reach for it only when the picture is
    /// genuinely universal. A glyph invented for a verb nobody draws is worse
    /// than the verb written out, because the reader now has to decode the
    /// icon *and* read the label to check they agree.
    ///
    /// The label always stays. The picture is the fast path for someone who
    /// already knows the control; the word is what makes it learnable, and it
    /// is the only part that can say "thirty seconds".
    #[must_use]
    pub fn controls<I, N, L>(mut self, columns: u8, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = cells.into_iter();
        let mut cells = Vec::new();
        for (name, label, glyph) in source.by_ref().take(MAX_CELLS) {
            cells.push(Cell::new(self.register(name.as_ref()), label).with_glyph(glyph));
        }
        if source.next().is_some() {
            self.warn_limit(id, "grid cells", MAX_CELLS);
        }
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square: false,
            cells,
        });
        self
    }

    /// Offers a value that moves one notch at a time.
    ///
    /// This is the shape a setting takes when its values form a line rather
    /// than a set: type size, brightness, playback speed. A list of named
    /// options would say the same thing in five rows of full-width boxes, and
    /// on a panel that repaints in tenths of a second the reader would rather
    /// tap the same spot twice than read five labels to find the one above the
    /// one they have.
    ///
    /// A table, drawn as columns that line up rather than as a sentence.
    ///
    /// Rows are given exactly as the document had them, headings included:
    /// the widths are worked out from all of them together, which is the only
    /// way the columns can agree, and that arithmetic belongs to the layout
    /// rather than to whoever is describing the page.
    #[must_use]
    pub fn table(mut self, rows: Vec<kobo_ui::TableRow>, weights: Vec<u16>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Table { id, rows, weights });
        self
    }

    /// The two ends carry pictures, not words, so the control needs no
    /// translating and no room for a label. Whichever end has nowhere further
    /// to go is drawn muted and stops answering taps.
    #[must_use]
    pub fn stepper(
        mut self,
        label: impl Into<String>,
        less: impl AsRef<str>,
        less_glyph: Glyph,
        more: impl AsRef<str>,
        more_glyph: Glyph,
    ) -> Self {
        let id = self.next_id();
        let less =
            BarAction::new(self.register(less.as_ref()), String::new()).with_glyph(less_glyph);
        let more =
            BarAction::new(self.register(more.as_ref()), String::new()).with_glyph(more_glyph);
        self.nodes.push(Node::Stepper {
            id,
            label: label.into(),
            less,
            more,
            less_state: ControlState::Enabled,
            more_state: ControlState::Enabled,
            fill: None,
        });
        self
    }

    /// Says which ends of the stepper just declared still have somewhere to go.
    #[must_use]
    pub fn stepper_ends(mut self, less: bool, more: bool) -> Self {
        if let Some(Node::Stepper {
            less_state,
            more_state,
            ..
        }) = self.nodes.last_mut()
        {
            *less_state = if less {
                ControlState::Enabled
            } else {
                ControlState::Disabled
            };
            *more_state = if more {
                ControlState::Enabled
            } else {
                ControlState::Disabled
            };
        }
        self
    }

    /// Draws a hairline under the stepper just declared showing where in its
    /// range the value sits, as a percentage of the way along.
    ///
    /// Worth having where the reading is a number without a natural sense of
    /// scale: "60%" says little until you can see it is past the middle.
    #[must_use]
    pub fn stepper_track(mut self, percent: u8) -> Self {
        if let Some(Node::Stepper { fill, .. }) = self.nodes.last_mut() {
            *fill = Some(percent.min(100));
        }
        self
    }

    /// Asks a question by offering answers.
    ///
    /// Prefer this over a text field. Typing on this device means summoning a
    /// keyboard onto a slow panel and hunting for keys, and it is markedly
    /// worse than tapping for anything that can be enumerated.
    #[must_use]
    pub fn choose<I, N, L>(mut self, prompt: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let mut source = options.into_iter();
        let mut options = Vec::new();
        for (name, label) in source.by_ref().take(MAX_CHOICE_OPTIONS) {
            options.push(BarAction::new(self.register(name.as_ref()), label));
        }
        if source.next().is_some() {
            self.warn_limit(id, "choice options", MAX_CHOICE_OPTIONS);
        }
        self.nodes.push(Node::Choice {
            id,
            prompt: prompt.into(),
            options,
            selected: None,
            freeform: None,
        });
        self
    }

    /// Adds the free-text escape hatch to the choice just declared.
    ///
    /// Deliberately a second call rather than a parameter, so that offering
    /// typing is a decision an author makes on purpose. The keyboard is only
    /// raised if the reader actually taps this row.
    #[must_use]
    pub fn or_type(mut self, name: impl AsRef<str>, placeholder: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Choice { freeform, .. }) = self.nodes.last_mut() {
            *freeform = Some(Freeform::new(action, placeholder));
        }
        self
    }

    /// Marks which option of the choice just declared is already the answer.
    ///
    /// State rather than decoration: the renderer draws the mark from the icon
    /// atlas, so an application never has to put a tick character in a label
    /// and never gets a missing-glyph box on a device whose face lacks it. An
    /// index naming no option leaves every row unmarked.
    #[must_use]
    pub fn chosen(mut self, index: usize) -> Self {
        if let Some(Node::Choice {
            options, selected, ..
        }) = self.nodes.last_mut()
        {
            *selected = u8::try_from(index)
                .ok()
                .filter(|index| usize::from(*index) < options.len());
        }
        self
    }

    /// Adds an attention strip.
    ///
    /// This is what to reach for instead of flashing the frontlight, which is a
    /// photosensitivity hazard and the largest power draw on the device.
    #[must_use]
    pub fn banner(mut self, level: BannerLevel, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Banner {
            id,
            level,
            text: text.into(),
        });
        self
    }

    /// Adds placeholder lines occupying the space real content will fill.
    ///
    /// Paint the real screen with these immediately and patch them as data
    /// arrives, rather than showing a splash. The panel is already displaying
    /// something at zero power, so there is no blank frame to cover.
    #[must_use]
    pub fn skeleton(mut self, lines: u8) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Skeleton { id, lines });
        self
    }

    /// A mark, a name and a sentence, centred in the room that is left.
    ///
    /// For the moment between asking for something and it arriving: opening an
    /// application, or a screen that exists only to say what is being waited
    /// on. Everything else on this platform is set ranged left from the top,
    /// which is right for reading and wrong for four words -- they land in the
    /// corner and read as a page that failed.
    ///
    /// Takes the rest of the content area, so put it last.
    #[must_use]
    pub fn splash(
        mut self,
        glyph: Option<Glyph>,
        title: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Splash {
            id,
            glyph,
            title: title.into(),
            summary: summary.into(),
        });
        self
    }

    /// States that work is in flight, for example a network request.
    ///
    /// The replacement for a spinner. Pass `None` for progress unless a real
    /// denominator is known; a bar that invents its own position is worse than
    /// no bar. Progress is snapped to coarse steps before it is drawn.
    #[must_use]
    pub fn activity(mut self, label: impl Into<String>, progress: Option<u8>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Activity {
            id,
            label: label.into(),
            progress: progress.map(Percent::new),
            cancel: None,
            transferred: None,
            failure: None,
        });
        self
    }

    /// States that bytes are arriving.
    ///
    /// `total` is the length the server announced, and `None` when it
    /// announced none. The distinction is the entire reason this exists: with
    /// a total you get a bar and "4.2 MB of 11 MB"; without one you get the
    /// count alone and no bar, because a progress bar that has invented its
    /// own denominator lies to the reader for as long as the download lasts.
    /// Byte counts are formatted by the renderer, so every application says
    /// "4.2 MB" the same way.
    ///
    /// Both numbers are **bytes**. Counting anything else with this -- stories
    /// fetched, messages sent -- captions the bar "3 B of 6 B". Use
    /// [`Self::activity`] with a percentage and say the count in the label.
    #[must_use]
    pub fn transfer(mut self, label: impl Into<String>, received: u64, total: Option<u64>) -> Self {
        let id = self.next_id();
        let progress = total.and_then(|total| {
            (total > 0).then(|| {
                let percent = received.saturating_mul(100) / total;
                Percent::new(u8::try_from(percent.min(100)).unwrap_or(100))
            })
        });
        // "4.2 MB" with no total is a truthful report of an unknown-length
        // download. "0 B" is not the same statement: it is the state every
        // such download begins in, it is what a reader sees for the whole of
        // a transfer the runtime hands over in one piece, and it reads as a
        // download that is failing rather than one that has not answered yet.
        // With nothing received and no total there is no amount to report, so
        // the label carries the screen alone.
        let transferred = (received > 0 || total.is_some()).then_some((received, total));
        self.nodes.push(Node::Activity {
            id,
            label: label.into(),
            progress,
            cancel: None,
            transferred,
            failure: None,
        });
        self
    }

    /// Says why the transfer just declared stopped.
    ///
    /// Attaches to the activity rather than replacing the screen, so whatever
    /// the reader was looking at is still there when it fails.
    #[must_use]
    pub fn transfer_failed(mut self, reason: impl Into<String>, resumable: bool) -> Self {
        if let Some(Node::Activity { failure, .. }) = self.nodes.last_mut() {
            *failure = Some(TransferFailure {
                reason: reason.into(),
                resumable,
            });
        }
        self
    }

    /// Offers to try again, but only if trying again could work.
    ///
    /// A no-op when the failure was not resumable. Offering a retry for
    /// something that can never succeed teaches readers that the controls on
    /// this device do nothing.
    #[must_use]
    pub fn transfer_retry(self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let resumable = matches!(
            self.nodes.last(),
            Some(Node::Activity {
                failure: Some(TransferFailure {
                    resumable: true,
                    ..
                }),
                ..
            })
        );
        if resumable {
            self.button(name, label)
        } else {
            self
        }
    }

    /// Lets the reader abandon the activity just declared.
    #[must_use]
    pub fn cancellable(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Activity { cancel, .. }) = self.nodes.last_mut() {
            *cancel = Some(BarAction::new(action, label));
        }
        self
    }

    #[must_use]
    pub fn build(self) -> Screen {
        Screen {
            id: self.id,
            top_bar: self.top_bar,
            nodes: self.nodes,
            nav_bar: self.nav_bar,
            bottom_action: self.bottom_action,
            page_turns: self.page_turns,
            hold: self.hold,
            owns_back: self.owns_back,
            text_scale: self.text_scale,
            overlay: self.overlay,
            reading: self.reading,
            // Applications built with this SDK emit the current protocol and
            // measure against Folio. Only a v11 decoder path marks legacy.
            legacy_typography: false,
            reading_font: self.reading_font,
        }
    }

    /// Returns warnings raised while bounded collections were added.
    ///
    /// Builders consume at most one item past each limit, so an accidental
    /// infinite iterator remains safe while the caller still learns that data
    /// was omitted.
    #[must_use]
    pub fn warnings(&self) -> &[LayoutIssue] {
        &self.warnings
    }

    /// Builds only when no rows, options, cells, or terminal lines were
    /// silently omitted.
    ///
    /// # Errors
    ///
    /// Returns every collection-limit warning raised while building. The
    /// ordinary [`Self::build`] remains available for compatibility.
    pub fn build_checked(self) -> Result<Screen, Vec<LayoutIssue>> {
        if self.warnings.is_empty() {
            Ok(self.build())
        } else {
            Err(self.warnings)
        }
    }

    fn register(&mut self, name: &str) -> ActionId {
        let action = action_id(name);
        if !self.actions.iter().any(|(known, _)| known == name) {
            self.actions.push((name.to_owned(), action));
        }
        action
    }

    fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node = self.next_node.saturating_add(1);
        id
    }

    /// Warns when a second bottom bar replaces the first.
    ///
    /// The panel has one bottom band and the last caller wins, silently. An
    /// application that called `action_bar` and then `nav_bar` -- which is
    /// what happens the moment a shared screen helper appends navigation --
    /// drew a screen with its verbs simply missing, and nothing anywhere said
    /// so.
    fn warn_second_bottom_bar(&mut self, id: NodeId) {
        if self.nav_bar.is_none() && self.bottom_action.is_none() {
            return;
        }
        self.warnings.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: Some(id),
            kind: LayoutIssueKind::CollectionTruncated {
                collection: "bottom bar",
                provided: 2,
                visible: 1,
            },
            rect: None,
        });
    }

    fn warn_limit(&mut self, id: NodeId, collection: &'static str, visible: usize) {
        self.warnings.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: Some(id),
            kind: LayoutIssueKind::CollectionTruncated {
                collection,
                provided: visible + 1,
                visible,
            },
            rect: None,
        });
    }
}

/// Deterministically maps an action name to a non-zero wire action ID.
#[must_use]
pub fn action_id(name: &str) -> ActionId {
    ActionId(stable_id(name))
}

fn stable_id(value: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in value.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    if hash == 0 {
        1
    } else {
        hash
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    SetScreen(Screen),
    Log {
        level: LogLevel,
        message: String,
    },
    Device(DeviceRequest),
    Spawn {
        task: TaskId,
        work: Task,
    },
    Cancel(TaskId),
    /// Read or write the application's own small state.
    Store(StoreRequest),
    /// Drive a terminal the runtime owns.
    Shell(ShellRequest),
    Exit,
    /// Hand the panel to another application by name.
    Launch(String),
    /// Give the runtime a picture to hold.
    PutPicture {
        handle: PictureHandle,
        width: u32,
        height: u32,
        grey: Vec<u8>,
    },
    /// Release a picture the runtime is holding.
    DropPicture(PictureHandle),
    /// Give the runtime a publisher font to hold for reading screens.
    PutFont {
        handle: FontHandle,
        name: String,
        bytes: Vec<u8>,
    },
    /// Release a publisher font and its glyph cache.
    DropFont(FontHandle),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    commands: Vec<Command>,
    next_task: u32,
    in_flight: usize,
    metrics: DisplayMetrics,
    /// Work the runner should try once more if it comes back retryable.
    ///
    /// Collected here and drained by the runner, because a context is built
    /// fresh for every callback and anything kept in it would be forgotten
    /// before the answer arrived.
    retrying: Vec<(TaskId, Task)>,
}

impl Context {
    /// The panel this application is drawing to.
    ///
    /// An application never positions anything, so this is not for layout. It
    /// is for the few decisions only the application can make, above all where
    /// to break a long document into pages, which depends on how much text the
    /// panel physically holds.
    #[must_use]
    pub const fn metrics(&self) -> DisplayMetrics {
        self.metrics
    }

    /// The same panel, measured at a different text size.
    ///
    /// For an application that sets [`ScreenBuilder::text_scale`]: paginate
    /// with this, at the same scale the screen asks for, or the page that was
    /// measured is not the page that gets drawn and its last paragraph is lost
    /// with nothing on the panel to say so.
    #[must_use]
    pub const fn metrics_at(&self, scale: kobo_ui::TextScale) -> DisplayMetrics {
        let mut metrics = self.metrics;
        metrics.text_scale = scale;
        metrics
    }

    /// Breaks prose into pages that fit this panel.
    ///
    /// Each page is a list of paragraphs to emit as separate text nodes, in
    /// order. Measuring is done with the runtime's own wrapping and line
    /// height, so a page that fits here is a page that will be drawn whole:
    /// layout stops at the bottom of the content area and silently drops the
    /// rest, which for a book means losing its last paragraph with nothing on
    /// the panel to say so.
    ///
    /// `nav_bar` says whether the reading screen pins its page controls to the
    /// bottom, which it should: controls at the end of the flow are the first
    /// thing a long page pushes off the panel.
    #[must_use]
    pub fn paginate(&self, text: &str, nav_bar: bool) -> Vec<Vec<String>> {
        kobo_ui::paginate(text, self.paged_area(nav_bar))
    }

    /// Breaks a book into pages, measured in the reading face.
    ///
    /// The companion to [`ScreenBuilder::reading`], and the only correct way to
    /// paginate for it: a serif sets the same words wider and on more generous
    /// lines, so a page measured in the interface face runs past the bottom of
    /// the one it is drawn on and loses its last lines with nothing on the
    /// panel to say so.
    #[must_use]
    pub fn paginate_reading(&self, text: &str, nav_bar: bool) -> Vec<Vec<String>> {
        kobo_ui::paginate(text, self.paged_area_in(nav_bar, kobo_ui::Face::Reading))
    }

    /// The same, at a text size other than the reader's own.
    ///
    /// Pair with [`ScreenBuilder::text_scale`] set to the same value.
    #[must_use]
    pub fn paginate_at(
        &self,
        text: &str,
        nav_bar: bool,
        scale: kobo_ui::TextScale,
    ) -> Vec<Vec<String>> {
        // Measured with the prose actually at that size. Setting it on the
        // metrics alone moves the margins and leaves the words the size they
        // were, which is how a page comes out measured for one size and drawn
        // at another. Only the prose moves: the bars above and below are
        // interface and keep the reader's own size, which is what makes the
        // page area the same whatever size the book is set at.
        kobo_ui::with_reading_scale(scale, || {
            let metrics = self.metrics_at(scale);
            let mut area = metrics.prose_area_in(true, nav_bar, kobo_ui::Face::Reading);
            area.height = area
                .height
                .saturating_sub(metrics.status_band_height())
                .saturating_sub(metrics.page_position_band())
                .max(1);
            kobo_ui::paginate(text, area)
        })
    }

    /// Breaks threaded prose into pages that fit this panel, keeping the
    /// depth of every paragraph.
    ///
    /// Indentation has to be measured, not applied afterwards: an indented
    /// paragraph is narrower, so it wraps to more lines and eats more of the
    /// page. Feed the result straight to [`ScreenBuilder::quote`].
    #[must_use]
    pub fn paginate_quoted(
        &self,
        paragraphs: &[(u8, QuoteRole, &str)],
        nav_bar: bool,
    ) -> Vec<Vec<(u8, QuoteRole, String)>> {
        kobo_ui::paginate_quoted(paragraphs, &self.metrics, self.paged_area(nav_bar))
    }

    /// The same, carrying a number of the application's choosing through the
    /// break so that a paragraph on a page can be traced back to what it came
    /// from. See [`kobo_ui::paginate_tagged`].
    #[must_use]
    pub fn paginate_tagged(
        &self,
        paragraphs: &[(u32, u8, QuoteRole, &str)],
        nav_bar: bool,
    ) -> Vec<Vec<(u32, u8, QuoteRole, String)>> {
        kobo_ui::paginate_tagged(paragraphs, &self.metrics, self.paged_area(nav_bar))
    }

    /// `text` cut to the single line a list row can show, ellipsised if it
    /// did not fit.
    ///
    /// A title that wraps makes its row taller than the one above it, and a
    /// list whose rows all differ in height is one the eye has to re-measure
    /// on every line. Measured against the same width the layout engine gives
    /// a row's words, so what fits here is what fits there.
    #[must_use]
    pub fn one_line_row(&self, text: &str, nav_bar: bool) -> String {
        self.clamped_row(text, 1, nav_bar)
    }

    /// `text` cut to at most `lines` lines of a list row.
    ///
    /// Two is the useful setting for anything written elsewhere (a headline, a
    /// subject line, a filename) because one line ellipsises most of them
    /// mid-sentence. Rows then differ in height, which [`Self::paginate_rows`]
    /// already accounts for.
    #[must_use]
    pub fn clamped_row(&self, text: &str, lines: usize, nav_bar: bool) -> String {
        let area = self.metrics.prose_area(true, nav_bar);
        kobo_ui::clamp_lines(
            text,
            kobo_ui::row_text_width(&self.metrics, area),
            kobo_ui::FontSize::Body,
            lines,
        )
    }

    /// The same, for a row that carries `trailing` at its trailing edge.
    ///
    /// The value keeps its column and the title gives up its own, so a title
    /// clamped at the full row width spills onto a third line beside a score.
    /// Measured against the width the title will really have.
    #[must_use]
    pub fn clamped_row_beside(
        &self,
        text: &str,
        trailing: &str,
        lines: usize,
        nav_bar: bool,
    ) -> String {
        let area = self.metrics.prose_area(true, nav_bar);
        kobo_ui::clamp_lines(
            text,
            kobo_ui::row_title_width(&self.metrics, area, trailing, false),
            kobo_ui::FontSize::Body,
            lines,
        )
    }

    /// `text` cut to one line of a row that carries an overflow mark.
    ///
    /// The mark keeps a finger's width of the row whatever the title says, so
    /// a title clamped at the full row width runs under the dots.
    #[must_use]
    pub fn one_line_row_with_menu(&self, text: &str, nav_bar: bool) -> String {
        let area = self.metrics.prose_area(true, nav_bar);
        kobo_ui::clamp_lines(
            text,
            kobo_ui::row_title_width(&self.metrics, area, "", true),
            kobo_ui::FontSize::Body,
            1,
        )
    }

    /// The content area an application screen actually gets.
    ///
    /// Less the status band, which the runtime draws above everything else and
    /// never tells the application about. `prose_area` describes the panel,
    /// not the screen: it knows about the top bar and the bottom one because
    /// the application asked for those, and nothing about the strip carrying
    /// the clock, the signal and the battery. Every paginator measured against
    /// the panel and came back with a page six millimetres taller than the one
    /// that would be drawn.
    ///
    /// This is the same band [`Chrome::measuring`] puts back for anything that
    /// measures a built screen, which is why the two disagreed.
    fn screen_area(&self, nav_bar: bool) -> ProseArea {
        self.screen_area_in(nav_bar, kobo_ui::Face::Text)
    }

    /// The same, in one face. A serif sets the same words on more generous
    /// lines, so a book measured in the interface face runs past the bottom of
    /// the page it is drawn on.
    fn screen_area_in(&self, nav_bar: bool, face: kobo_ui::Face) -> ProseArea {
        let mut area = self.metrics.prose_area_in(true, nav_bar, face);
        area.height = area
            .height
            .saturating_sub(self.metrics.status_band_height())
            .max(1);
        area
    }

    /// Breaks a list of rows into pages that fit this panel.
    ///
    /// Returns the row indices belonging to each page. Nothing in this UI
    /// scrolls: a panel that takes most of a second to repaint cannot follow a
    /// finger, so a list longer than the screen is turned like a page rather
    /// than dragged. Without this an application has no way to know where the
    /// fold is, and the layout engine simply stops drawing at the bottom.
    #[must_use]
    pub fn paginate_rows(&self, rows: &[(&str, &str)], nav_bar: bool) -> Vec<Vec<usize>> {
        kobo_ui::paginate_rows(rows, &self.metrics, self.paged_area(nav_bar))
    }

    /// The content area a screen that pages actually gets.
    ///
    /// Less the strip the page position takes, which the layout engine
    /// reserves before it places anything. Reserved here whether or not the
    /// caller ends up drawing a position, because everything that paginates
    /// turns pages, and the two mistakes are not equal: reserving a strip that
    /// is not used costs one line of white at the foot of the page, and not
    /// reserving one that is used costs the last row, drawn underneath the
    /// position and clipped by the bar.
    fn paged_area(&self, nav_bar: bool) -> ProseArea {
        self.paged_area_in(nav_bar, kobo_ui::Face::Text)
    }

    /// The same, in one face.
    fn paged_area_in(&self, nav_bar: bool, face: kobo_ui::Face) -> ProseArea {
        let mut area = self.screen_area_in(nav_bar, face);
        area.height = area
            .height
            .saturating_sub(self.metrics.page_position_band())
            .max(1);
        area
    }

    /// The same, for rows that carry a value at their trailing edge.
    ///
    /// Paired with [`ScreenBuilder::rows_with_trailing`]: the value keeps its
    /// column and the title gives up its own, so a list paginated as though
    /// the rows were full width comes back one row too many and the last one
    /// is drawn under the bottom bar.
    #[must_use]
    pub fn paginate_rows_with_trailing(
        &self,
        rows: &[(&str, &str, &str)],
        nav_bar: bool,
    ) -> Vec<Vec<usize>> {
        self.paginate_rows_with_trailing_at(rows, nav_bar, Position::AtTheFoot)
    }

    /// The same, for a ranked list, whose rows lead with digits.
    ///
    /// `highest` is the largest rank on show. Digits are narrower than a
    /// mark, and the layout engine draws the column at what sits in it, so a
    /// ranked list measured against a mark gives every title away and comes
    /// back a row short.
    #[must_use]
    pub fn paginate_ranked_rows_with_trailing(
        &self,
        rows: &[(&str, &str, &str)],
        nav_bar: bool,
        highest: u16,
        position: Position,
    ) -> Vec<Vec<usize>> {
        kobo_ui::paginate_ranked_rows_with_trailing(
            rows,
            &self.metrics,
            self.area_for(nav_bar, position),
            highest,
        )
    }

    /// The same, for a screen that says which page it is on somewhere else.
    ///
    /// The layout engine reserves the position strip only when there is a
    /// position to draw in it, so a screen that carries the count in its top
    /// bar and paginates as though the strip were there is handed a shorter
    /// page than it will be drawn on, and gives up the last row for a strip
    /// nobody reserved.
    #[must_use]
    pub fn paginate_rows_with_trailing_at(
        &self,
        rows: &[(&str, &str, &str)],
        nav_bar: bool,
        position: Position,
    ) -> Vec<Vec<usize>> {
        kobo_ui::paginate_rows_with_trailing(rows, &self.metrics, self.area_for(nav_bar, position))
    }

    /// The same, when one section header is drawn immediately above the rows.
    ///
    /// A section is content rather than chrome, so the panel area cannot
    /// reserve it automatically. Measuring the rows against the whole content
    /// area puts the final row underneath the bottom controls on a full page.
    /// This subtracts both the header and the inter-node gap the layout engine
    /// places after it.
    #[must_use]
    pub fn paginate_rows_with_trailing_after_section_at(
        &self,
        rows: &[(&str, &str, &str)],
        nav_bar: bool,
        position: Position,
    ) -> Vec<Vec<usize>> {
        let mut area = self.area_for(nav_bar, position);
        area.height = area
            .height
            .saturating_sub(kobo_ui::section_height(&self.metrics))
            .saturating_sub(area.gap)
            .max(1);
        kobo_ui::paginate_rows_with_trailing(rows, &self.metrics, area)
    }

    /// The page a list gets, given where it says which page that is.
    fn area_for(&self, nav_bar: bool, position: Position) -> kobo_ui::ProseArea {
        match position {
            Position::AtTheFoot => self.paged_area(nav_bar),
            Position::Elsewhere => self.screen_area(nav_bar),
        }
    }

    /// The same, for rows that carry an overflow mark against their right edge.
    #[must_use]
    pub fn paginate_rows_with_menu(&self, rows: &[(&str, &str)], nav_bar: bool) -> Vec<Vec<usize>> {
        kobo_ui::paginate_rows_with_menu(rows, &self.metrics, self.paged_area(nav_bar))
    }

    /// The same, where some rows open a new section.
    ///
    /// A section header is never left at the foot of a page with its first row
    /// overleaf. Pass `Some(title)` against the row a section begins at, and
    /// re-emit that title before the row when you draw the page.
    #[must_use]
    pub fn paginate_rows_in_sections(
        &self,
        rows: &[(Option<&str>, &str, &str)],
        nav_bar: bool,
    ) -> Vec<Vec<usize>> {
        kobo_ui::paginate_rows_in_sections(rows, &self.metrics, self.paged_area(nav_bar))
    }

    /// Breaks a grid of tiles into pages that fit this panel.
    ///
    /// Returns the tile indices belonging to each page. The count of tiles a
    /// panel holds is a measurement, not a constant: a Clara fits two columns
    /// and a Sage three, so an application that picked a number would silently
    /// lose its last entries on every panel but the one it was written on.
    ///
    /// Unlike a list, a grid is measured against the whole area rather than
    /// the area less the page position's strip. A grid clamps itself: cells
    /// are a fixed set on a panel that does not scroll, so when the room is
    /// short the layout shrinks the body rather than drawing past the bottom,
    /// and a row is never lost to a strip that was reserved and not used.
    /// Reserving it here is what cost the launcher a whole row of
    /// applications, because a grid's granularity is a third of the panel
    /// where a list's is one line.
    #[must_use]
    pub fn paginate_tiles(&self, count: usize, shape: TileShape, nav_bar: bool) -> Vec<Vec<usize>> {
        kobo_ui::paginate_tiles(count, &self.metrics, shape, self.screen_area(nav_bar))
    }

    /// Breaks a grid of tiles into pages that fit *under* what is already there.
    ///
    /// `placed` is the screen built with everything that precedes the grid --
    /// a band, a heading, a row. Its height is measured with the same engine
    /// that will draw it rather than estimated, because a grid that overruns
    /// loses its last tiles without a word, and the alternative in practice is
    /// to surrender a whole row of applications to be safe.
    #[must_use]
    pub fn paginate_tiles_under(
        &self,
        count: usize,
        shape: TileShape,
        nav_bar: bool,
        placed: &Screen,
    ) -> Vec<Vec<usize>> {
        let used = placed
            .layout_with(&self.metrics, &Chrome::measuring(true))
            .content_used();
        let mut area = self.screen_area(nav_bar);
        area.height = area
            .height
            .saturating_sub(used.saturating_add(area.gap))
            .max(1);
        kobo_ui::paginate_tiles(count, &self.metrics, shape, area)
    }

    /// Asks the runtime to hand the panel to another application.
    ///
    /// The name is looked up in the catalogue the runtime maintains; an
    /// application cannot name a path, so it cannot start anything that was not
    /// installed. Whether this is permitted at all is a capability, so an
    /// ordinary application asking for it is simply refused.
    ///
    /// This application stops when the other one starts and is started again
    /// when it finishes, so any state that must survive has to be saved first.
    pub fn launch(&mut self, name: impl Into<String>) {
        self.commands.push(Command::Launch(name.into()));
    }

    /// # Panics
    ///
    /// In debug builds only, on a screen the wire would refuse or one carrying
    /// a character the installed face cannot draw. Both are defects that are
    /// silent on the panel and obvious here.
    pub fn set_screen(&mut self, screen: Screen) {
        // A screen the wire refuses is not a rendering problem, it is a dead
        // connection: the runtime's reader stops at the malformed frame and
        // the application then waits forever for events from a socket nobody
        // is reading. On the panel that looks like every tap being ignored.
        // Checked in debug builds only, so an application's own tests fail on
        // the screen that built it rather than a device session doing nothing.
        debug_assert!(
            kobo_protocol::encode(&kobo_protocol::Frame {
                version: kobo_protocol::VERSION,
                request_id: 1,
                message: kobo_protocol::Message::SetScreen(screen.clone()),
            })
            .is_ok(),
            "this screen cannot be sent to the runtime: {:?}",
            kobo_protocol::encode(&kobo_protocol::Frame {
                version: kobo_protocol::VERSION,
                request_id: 1,
                message: kobo_protocol::Message::SetScreen(screen.clone()),
            })
            .err()
        );
        // The same idea one layer up: a character the installed face has no
        // glyph for is drawn as an empty box, which reads on the panel as a
        // broken renderer rather than as a missing character. Checked against
        // what will actually be drawn, so an application that marks its own
        // state with a symbol fails its own tests instead of shipping.
        #[cfg(debug_assertions)]
        {
            let layout = screen.layout_for(&self.metrics);
            for node in &layout.nodes {
                for line in &node.text_lines {
                    assert!(
                        kobo_ui::undrawable_in(line, kobo_ui::Face::Text).is_none(),
                        "this screen carries {:?}, which the installed face cannot draw: {line:?}",
                        kobo_ui::undrawable_in(line, kobo_ui::Face::Text).expect("just found one")
                    );
                }
            }
        }
        self.commands.push(Command::SetScreen(screen));
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.commands.push(Command::Log {
            level,
            message: message.into(),
        });
    }

    pub fn exit(&mut self) {
        self.commands.push(Command::Exit);
    }

    /// Hands a decoded picture to the runtime and returns the reference to put
    /// on a screen.
    ///
    /// Send a picture once and refer to it afterwards. Screens are re-sent
    /// whole on every change, so a picture that travelled with the screen would
    /// travel again on every tap.
    ///
    /// Fit the picture to the space it will occupy before calling this. The
    /// renderer will shrink an oversized one, but sending pixels that are
    /// immediately averaged away costs the wire, the runtime's cache and the
    /// battery for nothing.
    ///
    /// Returns `None` when the picture is empty, mis-sized, or larger than the
    /// bounded per-picture budget. Large pictures are chunked transparently by
    /// the socket client and become visible only after the final chunk arrives.
    pub fn put_picture(
        &mut self,
        handle: PictureHandle,
        width: u32,
        height: u32,
        grey: Vec<u8>,
    ) -> Option<TilePicture> {
        let expected = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))?;
        if expected == 0 || expected != grey.len() || expected > MAX_PICTURE_BYTES {
            return None;
        }
        self.commands.push(Command::PutPicture {
            handle,
            width,
            height,
            grey,
        });
        Some(TilePicture::new(handle, width, height))
    }

    /// Releases a picture. Every picture is released anyway when the
    /// application exits, so this is for one that outlives its usefulness.
    pub fn drop_picture(&mut self, handle: PictureHandle) {
        self.commands.push(Command::DropPicture(handle));
    }

    /// Hands a bounded embedded font to both local pagination and the runtime.
    ///
    /// Returns the handle only when the bytes are a supported TrueType or
    /// OpenType face. Unsupported WOFF data and malformed fonts leave the
    /// approved system reading face in place.
    pub fn put_font(
        &mut self,
        handle: FontHandle,
        name: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Option<FontHandle> {
        let name = name.into();
        if bytes.is_empty()
            || bytes.len() > MAX_FONT_BYTES
            || name.len() > kobo_protocol::MAX_STRING_LEN
        {
            return None;
        }
        #[cfg(feature = "text")]
        {
            let font = kobo_text::BookFont::from_bytes(&bytes, &name, self.metrics).ok()?;
            kobo_ui::put_book_typesetter(handle, Box::new(font));
        }
        self.commands.push(Command::PutFont {
            handle,
            name,
            bytes,
        });
        Some(handle)
    }

    /// Releases a publisher font locally and in the runtime.
    pub fn drop_font(&mut self, handle: FontHandle) {
        kobo_ui::drop_book_typesetter(handle);
        self.commands.push(Command::DropFont(handle));
    }

    /// Hands work to the runtime so the event loop keeps running.
    ///
    /// Returns `None` once too many tasks are already in flight, rather than
    /// queueing without limit. An application that cannot start more work
    /// should say so on screen; silently accumulating requests is how a device
    /// ends up holding the radio open with nothing to show for it.
    ///
    /// There is no blocking equivalent anywhere in this API. A blocking fetch
    /// would freeze the screen and the back control along with it.
    /// A task that cannot be put on the wire is refused the same way, because
    /// the alternative is worse: the encoder rejects the frame, the transport
    /// call returns the error, and the whole application ends with a protocol
    /// name printed at a terminal nobody on a reader is looking at. A request
    /// too large to send is the application's problem to show, not the
    /// runtime's problem to die of.
    pub fn spawn(&mut self, work: Task) -> Option<TaskId> {
        if self.in_flight >= MAX_TASKS_IN_FLIGHT || !work.is_sendable() {
            return None;
        }
        self.next_task = self.next_task.saturating_add(1);
        let task = TaskId(self.next_task);
        self.in_flight += 1;
        self.commands.push(Command::Spawn { task, work });
        Some(task)
    }

    /// Hands work to the runtime, and quietly tries it once more if the first
    /// attempt fails for a reason a second attempt could survive.
    ///
    /// This exists because of what a Kobo's radio does. It powers down when
    /// idle and wakes on demand, so the first request after the reader has
    /// been sitting on a page for a while routinely fails while the interface
    /// is still coming up, and succeeds a couple of seconds later. An
    /// application that reported that first failure would be telling the
    /// reader they are offline at the exact moment they are not.
    ///
    /// The retry is invisible. The second attempt reports back under the
    /// identifier returned here, so an application matches the answer to the
    /// request it made and never learns there were two. What it does learn is
    /// that a failure which reaches [`KoboApp::on_task`] has already been
    /// given its second chance, so it can say so on screen without hedging.
    ///
    /// Only [`TaskError::worth_retrying`] failures are tried again, and only
    /// once. A refused permission or a body too large is not going to change,
    /// and a reader watching a spinner is owed an answer rather than a loop.
    pub fn spawn_retrying(&mut self, work: Task) -> Option<TaskId> {
        let task = self.spawn(work.clone())?;
        self.retrying.push((task, work));
        Some(task)
    }

    /// Abandons a task. The application still receives exactly one
    /// [`KoboApp::on_task`] for it, reporting [`TaskOutcome::Cancelled`].
    pub fn cancel(&mut self, task: TaskId) {
        self.commands.push(Command::Cancel(task));
    }

    /// Records that a task has reported back, freeing one slot.
    pub(crate) fn settle(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    #[must_use]
    pub const fn tasks_in_flight(&self) -> usize {
        self.in_flight
    }

    /// Hardware operations, expressed as intent rather than device access.
    ///
    /// Every call queues one request. The runtime answers each one exactly
    /// once through [`KoboApp::on_device_result`], including when it refuses.
    pub fn device(&mut self) -> Device<'_> {
        Device { context: self }
    }

    /// Runtime-owned application catalog and package transactions.
    ///
    /// The runtime authorizes these requests by caller identity. A launcher
    /// may enumerate installed applications and the built-in Store may refresh
    /// and change them; ordinary applications receive an explicit refusal.
    /// Full Cobalt platform updates are deliberately absent and remain under
    /// [`Context::device`] for the Settings application.
    pub fn applications(&mut self) -> Applications<'_> {
        Applications { context: self }
    }

    /// Runtime-owned credentials that this application is allowed to use.
    ///
    /// A submitted value is written by the runtime and cannot be read back by
    /// the application. The runtime authorizes the calling app and exact
    /// secret name before replacing anything.
    pub fn secrets(&mut self) -> AppSecrets<'_> {
        AppSecrets { context: self }
    }

    /// The application's own small state, which survives being closed.
    ///
    /// Every application has one and none has to ask for it, in the same way a
    /// phone does not ask permission to remember which tab you were on. It is
    /// keyed, never pathed, so there is no syntax that can name somewhere else.
    pub fn store(&mut self) -> AppStore<'_> {
        AppStore { context: self }
    }

    /// The application's large data: books, covers, anything measured in
    /// megabytes rather than kilobytes.
    ///
    /// Separate from [`Context::store`] because the two fail differently. A
    /// state file is small enough to move in one message and small enough that
    /// losing it costs a reading position; a book is neither. See
    /// [`ShelfUpload`] and [`ShelfDownload`], which own the piecework.
    pub fn shelf(&mut self) -> AppShelf<'_> {
        AppShelf { context: self }
    }

    /// The terminal this application may run a program on.
    ///
    /// Nothing happens until [`AppShell::open`] is called, and nothing happens
    /// at all unless the application declared the `shell` capability. Like the
    /// network and the panel, the dangerous object stays behind the runtime:
    /// an application says what to type, never what to execute.
    pub fn shell(&mut self) -> AppShell<'_> {
        AppShell { context: self }
    }

    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    #[must_use]
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }
}

/// App-only catalog and installation operations.
#[derive(Debug)]
pub struct Applications<'a> {
    context: &'a mut Context,
}

/// Installation of app-scoped runtime credentials.
#[derive(Debug)]
pub struct AppSecrets<'a> {
    context: &'a mut Context,
}

impl AppSecrets<'_> {
    /// Installs or replaces one credential after the owner entered it.
    ///
    /// Completion or refusal is delivered through
    /// [`KoboApp::on_device_result`].
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.context
            .commands
            .push(Command::Device(DeviceRequest::SetSecret {
                name: name.into(),
                value: kobo_protocol::SecretValue::new(value.into()),
            }));
    }
}

impl Applications<'_> {
    /// Enumerates app-store packages currently installed on this reader.
    pub fn installed(&mut self) {
        self.request(DeviceRequest::ListInstalledApps);
    }

    /// Reads the last verified catalog without using the network.
    pub fn cached_catalog(&mut self) {
        self.request(DeviceRequest::ReadAppCatalog);
    }

    /// Fetches and verifies the fixed Cobalt app catalog.
    pub fn refresh_catalog(&mut self) {
        self.request(DeviceRequest::RefreshAppCatalog);
    }

    /// Installs or updates one catalog application.
    ///
    /// Returns `false` without queueing when `id` cannot be a stable
    /// application identity.
    pub fn install(&mut self, id: impl Into<String>) -> bool {
        self.with_id(id, |id| DeviceRequest::InstallApp { id })
    }

    /// Removes one app-store application. Application data is retained so a
    /// reinstall can recover it.
    pub fn uninstall(&mut self, id: impl Into<String>) -> bool {
        self.with_id(id, |id| DeviceRequest::UninstallApp { id })
    }

    fn with_id(
        &mut self,
        id: impl Into<String>,
        request: impl FnOnce(String) -> DeviceRequest,
    ) -> bool {
        let id = id.into();
        if !kobo_protocol::valid_app_id(&id) {
            return false;
        }
        self.request(request(id));
        true
    }

    fn request(&mut self, request: DeviceRequest) {
        self.context.commands.push(Command::Device(request));
    }
}

/// An application's own small state.
///
/// Sized for what an application needs in order to open where it closed: a
/// reading position, a list, a preference. Deliberately far too small for
/// content, which is what a task and a real file are for.
#[derive(Debug)]
pub struct AppStore<'a> {
    context: &'a mut Context,
}

impl AppStore<'_> {
    /// Begins a browser-link pairing attempt.
    pub fn begin_link(&mut self) {
        self.context
            .commands
            .push(Command::Device(DeviceRequest::BeginAppLink));
    }

    /// Reads the current browser-link state.
    pub fn read_link(&mut self) {
        self.context
            .commands
            .push(Command::Device(DeviceRequest::ReadAppLink));
    }

    /// Polls for pairing and remote installation progress.
    pub fn poll_link(&mut self) {
        self.context
            .commands
            .push(Command::Device(DeviceRequest::PollAppLink));
    }

    /// Disconnects every paired browser.
    pub fn disconnect_link(&mut self) {
        self.context
            .commands
            .push(Command::Device(DeviceRequest::DisconnectAppLink));
    }

    /// Writes a value, replacing whatever was under that key.
    ///
    /// The write is atomic: a reader sees the previous value or the new one and
    /// never a splice of the two, which matters on a device that loses power
    /// without warning.
    pub fn save(&mut self, key: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.request(StoreRequest::Save {
            key: key.into(),
            value: value.into(),
        });
    }

    /// Reads a value back. A key that was never written is not an error.
    pub fn load(&mut self, key: impl Into<String>) {
        self.request(StoreRequest::Load { key: key.into() });
    }

    /// Writes a value the runtime is free to throw away later.
    ///
    /// For anything that came from somewhere else and can come from there
    /// again: artwork, a rendered thumbnail, a parsed feed. Cache keys are
    /// counted and capped apart from ordinary ones ([`MAX_CACHE_KEYS`] of
    /// them), so caching a shelf of covers can never cost somebody their place
    /// in a book -- and a cache write is never refused, it makes room by
    /// dropping its own oldest entry instead.
    ///
    /// Never for anything that cannot be fetched a second time. It will be
    /// gone, and there will be no warning that it went.
    pub fn cache(&mut self, key: impl AsRef<str>, value: impl Into<Vec<u8>>) {
        self.save(cache_key(key), value);
    }

    /// Reads back something written with [`Self::cache`].
    ///
    /// A miss is [`StoreResult::Loaded`] with no value, exactly as for a key
    /// that was never written -- because after an eviction that is what it is.
    pub fn load_cached(&mut self, key: impl AsRef<str>) {
        self.load(cache_key(key));
    }

    /// Removes a key. Removing one that is not there is a success.
    pub fn forget(&mut self, key: impl Into<String>) {
        self.request(StoreRequest::Forget { key: key.into() });
    }

    /// Lists the keys this application has written.
    pub fn list(&mut self) {
        self.request(StoreRequest::List);
    }

    fn request(&mut self, request: StoreRequest) {
        self.context.commands.push(Command::Store(request));
    }
}

/// The key `cache` and `load_cached` actually use.
///
/// Public because an answer arrives carrying the key it was asked for, and an
/// application matching that answer has to be able to spell the same key.
#[must_use]
pub fn cache_key(key: impl AsRef<str>) -> String {
    format!("{CACHE_PREFIX}{}", key.as_ref())
}

/// An application's large data.
///
/// Everything here is a request answered through [`KoboApp::on_store`], like
/// the ordinary store. The difference is that a blob does not fit in one
/// message, so a write is a sequence and a read is a sequence -- which is what
/// [`ShelfUpload`] and [`ShelfDownload`] exist to stop every application
/// writing for itself.
#[derive(Debug)]
pub struct AppShelf<'a> {
    context: &'a mut Context,
}

impl AppShelf<'_> {
    /// Writes one piece of a blob at `offset`, finishing it when `last`.
    ///
    /// A blob cannot be read until a piece arrives with `last` set. That is
    /// deliberate: a half-downloaded book that opens is worse than no book,
    /// because it reads correctly until it stops mid-sentence and nothing
    /// distinguishes that from a book which was always like that.
    ///
    /// Prefer [`ShelfUpload`] to calling this directly.
    pub fn write(
        &mut self,
        name: impl Into<String>,
        offset: u32,
        bytes: impl Into<Vec<u8>>,
        last: bool,
    ) {
        self.request(StoreRequest::ShelfWrite {
            name: name.into(),
            offset,
            bytes: bytes.into(),
            last,
        });
    }

    /// Reads up to `length` bytes from `offset`. Prefer [`ShelfDownload`].
    pub fn read(&mut self, name: impl Into<String>, offset: u32, length: u32) {
        self.request(StoreRequest::ShelfRead {
            name: name.into(),
            offset,
            length,
        });
    }

    /// Removes a blob, and any half-written copy of it.
    pub fn remove(&mut self, name: impl Into<String>) {
        self.request(StoreRequest::ShelfRemove { name: name.into() });
    }

    /// Lists what this application has stored, with sizes.
    pub fn list(&mut self) {
        self.request(StoreRequest::ShelfList);
    }

    fn request(&mut self, request: StoreRequest) {
        self.context.commands.push(Command::Store(request));
    }
}

/// The most an application may pull back into memory in one download.
///
/// The device has 256 MB of RAM for everything, so a shelf that allows a
/// quarter of a gigabyte on the card must not allow the same figure in a
/// `Vec`. A book that is genuinely larger than this has to be read in pieces
/// by whatever knows how to page it, and this ceiling is where that decision
/// gets forced rather than discovered by an out-of-memory kill.
pub const MAX_SHELF_DOWNLOAD: usize = 32 * 1024 * 1024;

/// How a transfer answered the last result it was shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShelfProgress {
    /// Not this transfer's answer. Show it to whatever else is waiting.
    Elsewhere,
    /// Still going. `done` of `total` bytes have moved, and the next request
    /// has already been queued.
    Moving { done: u32, total: u32 },
    /// Finished.
    Done,
    /// Refused, and no further request was made.
    Failed(StoreError),
}

/// A clock for a screen that is waiting on something slow.
///
/// # Why an application needs one at all
///
/// Nothing else on this device ticks. The event loop is woken by taps, task
/// results and device answers, so a screen showing "Writing the spoken script"
/// against a request that takes a hundred seconds to send its first byte is
/// not slow to update, it is not updating at all. It is pixel for pixel
/// identical to the same application having crashed, and the only honest
/// difference is that one of them will eventually change.
///
/// So this borrows the one thing the runtime already has that finishes on a
/// schedule: a sleep task. Each nap that ends is a tick, and each tick is
/// re-armed until somebody stops it.
///
/// # What to do with the tick
///
/// Show [`Self::waited`]. Not a made-up percentage that advances on a timer,
/// which is a lie the moment the request finishes early or late, but the plain
/// count of how long this has been going. It is the one number that is always
/// true, and it is enough: a reader who can see the wait growing knows the
/// reader is alive, and a reader who can see it has reached four minutes knows
/// to press Cancel.
///
/// # What it costs
///
/// One partial refresh per tick. Five seconds is the default because it is
/// slow enough that a two minute wait costs two dozen repaints rather than a
/// hundred, and fast enough that the panel is never still for long enough to
/// look dead.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    task: Option<TaskId>,
    seconds: u32,
    waited: u32,
}

/// How often a clock ticks when nobody says otherwise.
pub const DEFAULT_HEARTBEAT_SECONDS: u32 = 5;

/// Where a paginated screen tells the reader which page they are on.
///
/// The default is the foot of the list, which is the strip the layout engine
/// reserves and draws the turns' chevrons in. A screen that has already said
/// it in its top bar wants [`Position::Elsewhere`]: saying it twice is two
/// answers to one question, and paying for the strip anyway costs the list a
/// row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Position {
    /// In the strip under the list.
    #[default]
    AtTheFoot,
    /// Anywhere but there, so no strip is reserved.
    Elsewhere,
}

/// Written out rather than derived, because a derived one would tick every
/// zero seconds. That is not a slow clock, it is a spin: the runtime completes
/// the nap immediately, the tick re-arms it, and the application asks for four
/// hundred sleeps in the time one request takes. Every application that holds
/// a clock in a `#[derive(Default)]` state struct gets it this way, so the
/// default has to be the sensible cadence rather than the zero value.
impl Default for Heartbeat {
    fn default() -> Self {
        Self::every(DEFAULT_HEARTBEAT_SECONDS)
    }
}

impl Heartbeat {
    /// A clock that ticks every `seconds`, not yet running.
    #[must_use]
    pub const fn every(seconds: u32) -> Self {
        Self {
            task: None,
            seconds: if seconds == 0 { 1 } else { seconds },
            waited: 0,
        }
    }

    /// Starts ticking from zero. Starting one that is already running does
    /// nothing, so this is safe to call on every stage change.
    pub fn start(&mut self, context: &mut Context) {
        if self.task.is_some() {
            return;
        }
        self.waited = 0;
        self.arm(context);
    }

    /// Stops ticking, and forgets the wait.
    ///
    /// The nap is cancelled rather than left to expire, because a tick that
    /// arrives after the thing it was timing has finished would re-arm itself
    /// against a screen nobody is waiting on.
    pub fn stop(&mut self, context: &mut Context) {
        if let Some(task) = self.task.take() {
            context.cancel(task);
        }
        self.waited = 0;
    }

    /// Whether `task` was this clock's, re-arming it if so.
    ///
    /// Call it first in [`KoboApp::on_task`] and return early when it is
    /// `true`: a tick is not the application's answer and must not be matched
    /// against whatever the application is waiting for.
    pub fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: &TaskOutcome) -> bool {
        if self.task != Some(task) {
            return false;
        }
        self.task = None;
        if matches!(outcome, TaskOutcome::Cancelled) {
            return true;
        }
        self.waited = self.waited.saturating_add(self.seconds);
        self.arm(context);
        true
    }

    /// How long this has been waiting, to the tick.
    #[must_use]
    pub const fn waited(&self) -> Duration {
        Duration::from_secs(self.waited as u64)
    }

    /// Whether the clock is running.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.task.is_some()
    }

    /// The wait as words, for the line under an activity label.
    ///
    /// Empty until the first tick, so a screen that has only just appeared
    /// does not flash "0 seconds" at somebody before it has waited for
    /// anything at all.
    #[must_use]
    pub fn waited_words(&self) -> String {
        match self.waited {
            0 => String::new(),
            seconds if seconds < 60 => format!("{seconds} seconds so far"),
            seconds => {
                let (minutes, rest) = (seconds / 60, seconds % 60);
                if rest == 0 {
                    format!("{minutes} min so far")
                } else {
                    format!("{minutes} min {rest} s so far")
                }
            }
        }
    }

    fn arm(&mut self, context: &mut Context) {
        self.task = context.spawn(Task::Sleep {
            seconds: self.seconds.max(1),
        });
    }
}

/// Writes a blob to the shelf in as many pieces as it takes.
///
/// # Why the runtime's answer drives the next request
///
/// Each answer carries how much of the blob exists, and the next piece is cut
/// from there rather than from a cursor kept on this side. Two counters that
/// are supposed to agree eventually do not, and the failure that produces is a
/// book with a gap in the middle -- which opens, reads, and is quietly wrong.
/// One counter cannot disagree with itself.
#[derive(Clone, Debug)]
pub struct ShelfUpload {
    name: String,
    bytes: Vec<u8>,
    sent: u32,
}

impl ShelfUpload {
    #[must_use]
    pub fn new(name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            bytes: bytes.into(),
            sent: 0,
        }
    }

    /// Queues the first piece. Beginning at zero also discards anything left
    /// from an earlier attempt that never finished.
    pub fn start(&mut self, context: &mut Context) {
        self.sent = 0;
        self.send(context);
    }

    /// Feeds this transfer one store answer.
    pub fn advance(&mut self, context: &mut Context, result: &StoreResult) -> ShelfProgress {
        match result {
            StoreResult::ShelfWritten { name, size } if *name == self.name => {
                self.sent = *size;
                if usize::try_from(*size).unwrap_or(usize::MAX) >= self.bytes.len() {
                    ShelfProgress::Done
                } else {
                    self.send(context);
                    ShelfProgress::Moving {
                        done: *size,
                        total: self.total(),
                    }
                }
            }
            StoreResult::Denied(error) => ShelfProgress::Failed(*error),
            _ => ShelfProgress::Elsewhere,
        }
    }

    fn send(&mut self, context: &mut Context) {
        let from = usize::try_from(self.sent)
            .unwrap_or(usize::MAX)
            .min(self.bytes.len());
        let to = from.saturating_add(MAX_SHELF_CHUNK).min(self.bytes.len());
        let piece = self.bytes[from..to].to_vec();
        let last = to == self.bytes.len();
        context
            .shelf()
            .write(self.name.clone(), self.sent, piece, last);
    }

    fn total(&self) -> u32 {
        u32::try_from(self.bytes.len()).unwrap_or(u32::MAX)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Reads a blob back from the shelf in as many pieces as it takes.
#[derive(Clone, Debug)]
pub struct ShelfDownload {
    name: String,
    bytes: Vec<u8>,
    limit: usize,
}

impl ShelfDownload {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            bytes: Vec::new(),
            limit: MAX_SHELF_DOWNLOAD,
        }
    }

    /// Stops after `limit` bytes rather than [`MAX_SHELF_DOWNLOAD`].
    #[must_use]
    pub fn at_most(mut self, limit: usize) -> Self {
        self.limit = limit.min(MAX_SHELF_DOWNLOAD);
        self
    }

    /// Queues the first read.
    pub fn start(&mut self, context: &mut Context) {
        self.bytes.clear();
        self.request(context);
    }

    /// Feeds this transfer one store answer.
    pub fn advance(&mut self, context: &mut Context, result: &StoreResult) -> ShelfProgress {
        let StoreResult::ShelfRead {
            name,
            offset,
            bytes,
            size,
        } = result
        else {
            return match result {
                StoreResult::Denied(error) => ShelfProgress::Failed(*error),
                _ => ShelfProgress::Elsewhere,
            };
        };
        if *name != self.name {
            return ShelfProgress::Elsewhere;
        }
        // A piece that does not begin where the last one ended would splice
        // the blob together wrongly. Refusing is the only safe answer: the
        // result would be a file that parses and is not the file.
        if usize::try_from(*offset).unwrap_or(usize::MAX) != self.bytes.len() {
            return ShelfProgress::Failed(StoreError::Missing);
        }
        if usize::try_from(*size).unwrap_or(usize::MAX) > self.limit {
            return ShelfProgress::Failed(StoreError::TooFull);
        }
        self.bytes.extend_from_slice(bytes);
        let done = u32::try_from(self.bytes.len()).unwrap_or(u32::MAX);
        if done >= *size {
            return ShelfProgress::Done;
        }
        // An empty piece before the end would otherwise ask again forever.
        if bytes.is_empty() {
            return ShelfProgress::Failed(StoreError::Missing);
        }
        self.request(context);
        ShelfProgress::Moving { done, total: *size }
    }

    fn request(&mut self, context: &mut Context) {
        let offset = u32::try_from(self.bytes.len()).unwrap_or(u32::MAX);
        let length = u32::try_from(MAX_SHELF_CHUNK).unwrap_or(u32::MAX);
        context.shelf().read(self.name.clone(), offset, length);
    }

    /// What has arrived so far, which is the whole blob once
    /// [`ShelfProgress::Done`] has been returned.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn take(self) -> Vec<u8> {
        self.bytes
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The application's terminal.
///
/// Every method here is a request, answered through
/// [`KoboApp::on_shell_event`]. There is no return value to check because
/// there is nothing to check yet: the runtime may refuse, the program may fail
/// to start, and either way the answer arrives as an event like any other.
#[derive(Debug)]
pub struct AppShell<'a> {
    context: &'a mut Context,
}

impl AppShell<'_> {
    /// Starts a program on a terminal of exactly this grid.
    ///
    /// Ask [`terminal_grid_for`] what the grid is rather than choosing one.
    /// A program told a width the panel does not have wraps its lines in a
    /// different place from where the reader sees them wrap, which makes every
    /// full-screen program unusable.
    pub fn open(&mut self, columns: u16, rows: u16) {
        self.request(ShellRequest::Open { columns, rows });
    }

    /// Sends keystrokes, already encoded as the bytes a terminal expects.
    pub fn input(&mut self, bytes: impl Into<Vec<u8>>) {
        self.request(ShellRequest::Input(bytes.into()));
    }

    /// Tells the program the grid changed.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.request(ShellRequest::Resize { columns, rows });
    }

    /// Ends the program.
    pub fn close(&mut self) {
        self.request(ShellRequest::Close);
    }

    fn request(&mut self, request: ShellRequest) {
        self.context.commands.push(Command::Shell(request));
    }
}

/// Capability-gated hardware operations.
///
/// An application never opens a device node, chooses a waveform, writes a sysfs
/// file, or talks to a radio. It states what it wants; the runtime decides.
/// Durations are advisory upper bounds and are clamped by system policy, so a
/// grant may be shorter than the request.
#[derive(Debug)]
pub struct Device<'a> {
    context: &'a mut Context,
}

impl Device<'_> {
    /// Asks for the battery percentage and charging state.
    pub fn read_battery(&mut self) {
        self.request(DeviceRequest::ReadBattery);
    }

    /// Asks for everything the gauge publishes: health, wear, temperature,
    /// voltage, current and time remaining.
    ///
    /// Ask this when a reader has opened a screen about the battery and is
    /// looking at it. Every field comes back optional, because these are a
    /// vendor driver's choice rather than a kernel guarantee, so show what
    /// arrives rather than reserving space for what might.
    pub fn read_battery_detail(&mut self) {
        self.request(DeviceRequest::ReadBatteryDetail);
    }

    /// Asks what this runtime is and what it is running on.
    ///
    /// The answer arrives as [`DeviceResult::Identity`] and carries the
    /// matched profile, model, firmware, kernel, panel size and runtime
    /// version. Ask this when a reader opens a screen that shows them, such
    /// as an about page whose photograph serves as evidence that a build ran
    /// on real hardware.
    pub fn read_identity(&mut self) {
        self.request(DeviceRequest::ReadIdentity);
    }

    /// Asks where the magnet is now.
    ///
    /// Changes arrive on their own through [`KoboApp::on_cover_change`]. This
    /// is for the screen that has just opened and has no state to change from.
    pub fn read_cover(&mut self) {
        self.request(DeviceRequest::ReadCover);
    }

    /// Asks to keep Wi-Fi associated for at most `duration`.
    ///
    /// Use this for a dashboard that must stay reachable. It is the most
    /// expensive thing an application can ask for, so expect a shorter grant
    /// than requested and a refusal on a low battery.
    pub fn hold_wifi(&mut self, duration: Duration) {
        self.request(DeviceRequest::HoldWifi {
            seconds: whole_seconds(duration),
        });
    }

    /// Releases a Wi-Fi hold before it expires.
    pub fn release_wifi(&mut self) {
        self.request(DeviceRequest::ReleaseWifi);
    }

    /// Asks to stay out of suspend for at most `duration`.
    pub fn keep_awake(&mut self, duration: Duration) {
        self.request(DeviceRequest::KeepAwake {
            seconds: whole_seconds(duration),
        });
    }

    /// Releases a wake hold before it expires.
    pub fn allow_sleep(&mut self) {
        self.request(DeviceRequest::AllowSleep);
    }

    /// Asks to be woken after `delay` to refresh content.
    ///
    /// The runtime coalesces wakes across applications and enforces a minimum
    /// interval, so the granted delay is often longer than requested.
    pub fn schedule_wake(&mut self, delay: Duration) {
        self.request(DeviceRequest::ScheduleWake {
            seconds: whole_seconds(delay),
        });
    }

    /// Cancels a pending scheduled wake.
    pub fn cancel_wake(&mut self) {
        self.request(DeviceRequest::CancelWake);
    }

    /// Sets the front light, as a percentage of its range.
    pub fn set_frontlight(&mut self, percent: u8) {
        self.request(DeviceRequest::SetFrontlight {
            percent: percent.min(100),
        });
    }

    /// Asks for the current front light percentage.
    pub fn read_frontlight(&mut self) {
        self.request(DeviceRequest::ReadFrontlight);
    }

    /// Asks whether Bluetooth is available and powered, including remembered
    /// devices when the backend can enumerate them without scanning.
    pub fn read_bluetooth(&mut self) {
        self.request(DeviceRequest::ReadBluetooth);
    }

    /// Powers Bluetooth on or off.
    pub fn set_bluetooth(&mut self, enabled: bool) {
        self.request(DeviceRequest::SetBluetooth { enabled });
    }

    /// Starts a bounded discovery and returns the resulting device list.
    pub fn scan_bluetooth(&mut self) {
        self.request(DeviceRequest::ScanBluetooth);
    }

    /// Pairs with `address`. Returns `false` without queueing when the address
    /// is not a canonical six-byte Bluetooth address.
    pub fn pair_bluetooth(&mut self, address: impl Into<String>) -> bool {
        self.bluetooth_address(address, |address| DeviceRequest::PairBluetooth { address })
    }

    /// Connects a Bluetooth device.
    pub fn connect_bluetooth(&mut self, address: impl Into<String>) -> bool {
        self.bluetooth_address(address, |address| DeviceRequest::ConnectBluetooth {
            address,
        })
    }

    /// Disconnects a Bluetooth device without forgetting the pairing.
    pub fn disconnect_bluetooth(&mut self, address: impl Into<String>) -> bool {
        self.bluetooth_address(address, |address| DeviceRequest::DisconnectBluetooth {
            address,
        })
    }

    /// Removes a remembered Bluetooth pairing.
    pub fn forget_bluetooth(&mut self, address: impl Into<String>) -> bool {
        self.bluetooth_address(address, |address| DeviceRequest::ForgetBluetooth {
            address,
        })
    }

    /// Asks for Wi-Fi power and association state.
    pub fn read_wifi(&mut self) {
        self.request(DeviceRequest::ReadWifi);
    }

    /// Powers the Wi-Fi interface on or off.
    pub fn set_wifi(&mut self, enabled: bool) {
        self.request(DeviceRequest::SetWifi { enabled });
    }

    /// Scans for nearby Wi-Fi networks.
    pub fn scan_wifi(&mut self) {
        self.request(DeviceRequest::ScanWifi);
    }

    /// Joins a Wi-Fi network. Pass an empty password for an open network.
    /// Returns `false` without queueing malformed credentials.
    pub fn join_wifi(&mut self, ssid: impl Into<String>, password: impl Into<String>) -> bool {
        let ssid = ssid.into();
        let password = password.into();
        if ssid.is_empty()
            || ssid.len() > 32
            || !(password.is_empty() || (8..=63).contains(&password.len()))
        {
            return false;
        }
        self.request(DeviceRequest::JoinWifi { ssid, password });
        true
    }

    /// Leaves the current network without powering Wi-Fi off.
    pub fn disconnect_wifi(&mut self) {
        self.request(DeviceRequest::DisconnectWifi);
    }

    /// Reads the active audio transport state and position.
    pub fn read_audio(&mut self) {
        self.request(DeviceRequest::ReadAudio);
    }

    /// Prepares an audio source without starting playback.
    pub fn load_audio(&mut self, source: AudioSource) {
        self.request(DeviceRequest::LoadAudio { source });
    }

    /// Prepares a file in this application's shelf.
    ///
    /// Returns `false` without queueing when `name` is not a valid shelf key.
    pub fn load_shelf_audio(&mut self, name: impl Into<String>) -> bool {
        let name = name.into();
        if !kobo_protocol::is_valid_key(&name) {
            return false;
        }
        self.load_audio(AudioSource::Shelf(name));
        true
    }

    /// Prepares an unauthenticated HTTPS audio stream.
    ///
    /// Returns `false` without queueing for a malformed or oversized URL.
    pub fn load_audio_stream(&mut self, url: impl Into<String>) -> bool {
        let url = url.into();
        if !url.starts_with("https://") || url.len() > MAX_URL_LEN {
            return false;
        }
        self.load_audio(AudioSource::Stream(url));
        true
    }

    /// Starts or resumes the prepared audio source.
    pub fn play_audio(&mut self) {
        self.request(DeviceRequest::PlayAudio);
    }

    /// Pauses playback at the current position.
    pub fn pause_audio(&mut self) {
        self.request(DeviceRequest::PauseAudio);
    }

    /// Seeks to an absolute position from the start of the source.
    pub fn seek_audio(&mut self, position: Duration) {
        let millis = position.as_millis();
        self.request(DeviceRequest::SeekAudio {
            position_ms: u32::try_from(millis).unwrap_or(u32::MAX),
        });
    }

    /// Stops playback and returns the prepared source to its beginning.
    pub fn stop_audio(&mut self) {
        self.request(DeviceRequest::StopAudio);
    }

    /// Sets software playback volume, clamped to 0–100 percent.
    pub fn set_audio_volume(&mut self, percent: u8) {
        self.request(DeviceRequest::SetAudioVolume {
            percent: percent.min(100),
        });
    }

    /// Looks up one word using runtime-installed dictionaries without opening
    /// the radio. The answer arrives through `on_device_result` as
    /// `DeviceResult::Dictionary`, including an explicit empty result.
    pub fn lookup_word(
        &mut self,
        word: impl Into<String>,
        language: Option<impl Into<String>>,
    ) -> bool {
        let word = word.into();
        let language = language.map(Into::into);
        if word.trim().is_empty()
            || word.len() > MAX_LOOKUP_WORD_BYTES
            || language.as_deref().is_some_and(|language| {
                language.is_empty()
                    || language.len() > 16
                    || !language
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        {
            return false;
        }
        self.request(DeviceRequest::LookupWord { word, language });
        true
    }

    /// Replaces the installed Cobalt with a published release archive.
    ///
    /// The runtime downloads `url`, refuses the bytes unless their SHA-256
    /// digest is exactly `sha256`, and swaps the new install into place while
    /// keeping the old one beside it. The reply is [`DeviceResult::Done`] or
    /// a [`DeviceResult::Failed`] naming what went wrong.
    ///
    /// Returns `false` without queueing for a malformed URL or a string that
    /// is not a sixty-four character lowercase hex digest.
    pub fn update(&mut self, url: impl Into<String>, sha256: impl Into<String>) -> bool {
        let url = url.into();
        let sha256 = sha256.into();
        if !url.starts_with("https://") || url.len() > MAX_URL_LEN {
            return false;
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return false;
        }
        self.request(DeviceRequest::Update { url, sha256 });
        true
    }

    /// Asks which automatic updates the runtime performs on its own. The
    /// reply is [`DeviceResult::AutoUpdate`] with one switch for the platform
    /// and one for installed applications.
    ///
    /// Behind the `runtime-settings` switch, because the runtime answers this
    /// only for the settings application it ships with. See that feature in
    /// this crate's manifest for why it is not offered to everyone.
    #[cfg(feature = "runtime-settings")]
    pub fn read_auto_update(&mut self) {
        self.request(DeviceRequest::ReadAutoUpdate);
    }

    /// Chooses which automatic updates the runtime performs on its own. Both
    /// switches are stated together so two screens cannot interleave writes
    /// and leave a mixture neither of them chose. The reply restates what was
    /// recorded, as [`DeviceResult::AutoUpdate`].
    ///
    /// Behind the `runtime-settings` switch, for the same reason as
    /// [`Self::read_auto_update`].
    #[cfg(feature = "runtime-settings")]
    pub fn set_auto_update(&mut self, cobalt: bool, apps: bool) {
        self.request(DeviceRequest::SetAutoUpdate { cobalt, apps });
    }

    /// Asks which stable or beta release channel the runtime follows. The
    /// reply is [`DeviceResult::UpdateChannel`].
    #[cfg(feature = "runtime-settings")]
    pub fn read_update_channel(&mut self) {
        self.request(DeviceRequest::ReadUpdateChannel);
    }

    /// Atomically selects the stable or beta release channel used for both
    /// platform and Store app updates.
    #[cfg(feature = "runtime-settings")]
    pub fn set_update_channel(&mut self, channel: UpdateChannel) {
        self.request(DeviceRequest::SetUpdateChannel { channel });
    }

    fn bluetooth_address(
        &mut self,
        address: impl Into<String>,
        request: impl FnOnce(String) -> DeviceRequest,
    ) -> bool {
        let address = address.into();
        if !is_bluetooth_address(&address) {
            return false;
        }
        self.request(request(address));
        true
    }

    fn request(&mut self, request: DeviceRequest) {
        self.context.commands.push(Command::Device(request));
    }
}

fn is_bluetooth_address(address: &str) -> bool {
    let mut parts = address.split(':');
    let valid = (&mut parts)
        .take(6)
        .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_hexdigit()));
    valid && parts.next().is_none() && address.matches(':').count() == 5
}

/// Converts a duration to whole seconds without overflowing or rounding to zero.
fn whole_seconds(duration: Duration) -> u32 {
    let seconds = duration.as_secs();
    if seconds == 0 && duration.subsec_nanos() > 0 {
        return 1;
    }
    u32::try_from(seconds).unwrap_or(u32::MAX)
}

/// Application lifecycle driven by the embedding platform.
pub trait KoboApp {
    fn on_start(&mut self, context: &mut Context);
    fn on_action(&mut self, context: &mut Context, action: ActionId);

    /// Receives a held word in stable application text coordinates.
    /// Applications that have not opted into selection retain the historical
    /// hold action behavior.
    fn on_text_hold(&mut self, context: &mut Context, action: ActionId, _hit: TextHit) {
        self.on_action(context, action);
    }

    fn on_resume(&mut self, _context: &mut Context) {}

    fn on_suspend(&mut self, _context: &mut Context) {}

    fn on_scheduled_wake(&mut self, _context: &mut Context) {}

    fn on_exit(&mut self, _context: &mut Context) {}

    /// Receives the runtime's answer to exactly one earlier device request.
    ///
    /// Every request produces exactly one call, so an application never has to
    /// guess whether something was honoured.
    fn on_device_result(
        &mut self,
        _context: &mut Context,
        _request: DeviceRequest,
        _result: DeviceResult,
    ) {
    }

    /// A magnet arrived at, or left, the reader's hall sensor.
    ///
    /// The sensor is behind one edge of the bezel and is what a sleep cover
    /// closes against, but it cannot tell a cover from any other magnet, so
    /// this reports what was measured rather than what it might mean.
    ///
    /// Only changes arrive, and only real ones: the sensor bounces while a
    /// magnet is moved slowly past it, and the runtime settles that before
    /// telling anyone. Ask [`Device::read_cover`] for the state to start from.
    fn on_cover_change(&mut self, _context: &mut Context, _magnet_present: bool) {}

    /// Receives a physical page-turn key press, already resolved to intent.
    ///
    /// The runtime owns the raw keycodes and knows how the reader is held;
    /// an application only hears which way the reader wants to go. Doing
    /// nothing here is the default and is honest: there is no runtime
    /// fallback for an application that ignores its buttons.
    fn on_page_turn(&mut self, _context: &mut Context, _forward: bool) {}

    /// Receives the outcome of exactly one earlier [`Context::spawn`].
    ///
    /// Like device results, a task always reports back, including when it fails
    /// or is cancelled, so an application never has to time out its own work.
    fn on_task(&mut self, _context: &mut Context, _task: TaskId, _outcome: TaskOutcome) {}

    /// The reader left this application for another one.
    ///
    /// Nothing is stopped: work in flight keeps running and answers keep
    /// arriving. What changes is that nothing drawn from here will be seen
    /// until [`KoboApp::on_foreground`], so this is the moment to write
    /// anything that would be missed if the device never came back.
    fn on_background(&mut self, _context: &mut Context) {}

    /// The reader came back.
    ///
    /// The panel still holds whatever was last drawn from this application, so
    /// there is no blank to cover, but anything that changed while it was away
    /// has to be drawn now.
    fn on_foreground(&mut self, _context: &mut Context) {}

    /// Receives the runtime's answer to exactly one earlier store request.
    ///
    /// Like device results, every request reports back, so an application never
    /// has to guess whether its state was written.
    fn on_store(&mut self, _context: &mut Context, _result: StoreResult) {}

    /// Receives everything a terminal has to say: that it opened, what the
    /// program printed, that it finished, or that the request was refused.
    fn on_shell_event(&mut self, _context: &mut Context, _event: ShellEvent) {}
}

/// The longest a lifecycle callback may run before the runtime intervenes.
///
/// This exists because an application that blocks in a callback would otherwise
/// hold the only thread that can repaint the screen, read a touch, or honour a
/// request to leave. That is a safety problem rather than a style one: the
/// reader's only remaining option would be a hard power cycle.
pub const CALLBACK_DEADLINE: Duration = Duration::from_millis(250);

/// The ceiling on tasks in flight for one application at once.
///
/// Each task is a real connection or file handle, and an unbounded queue is an
/// unbounded amount of radio time.
pub const MAX_TASKS_IN_FLIGHT: usize = 4;

/// How long the runner waits before trying failed work a second time.
///
/// Long enough for a Kobo's radio to finish coming up, which is the failure
/// this retry exists for, and short enough that a reader watching a spinner
/// does not conclude the application has hung.
pub const RETRY_DELAY_SECONDS: u32 = 3;

#[derive(Debug)]
pub struct AppRunner<A> {
    app: A,
    metrics: DisplayMetrics,
    started: bool,
    pending: VecDeque<DeviceRequest>,
    /// Store requests sent but not yet answered. Every request is answered
    /// exactly once, so a count is enough and the request itself need not be
    /// kept: unlike a device answer, a store answer names its own key.
    pending_stores: usize,
    /// Task counters live here rather than in `Context`, because a fresh
    /// context is built for every callback. Left in the context they would
    /// restart at one on each dispatch, so the second callback to spawn work
    /// would hand out an identifier already in use and the two tasks would
    /// report back as one.
    next_task: u32,
    in_flight: usize,
    settled: bool,
    /// Work handed over by [`Context::spawn_retrying`] that has not yet been
    /// given its second chance, keyed by the identifier the application holds.
    retrying: BTreeMap<TaskId, Task>,
    /// Naps between a first failure and a second attempt, each remembering the
    /// identifier the application is waiting on and the work to try again.
    napping: BTreeMap<TaskId, (TaskId, Task)>,
    /// Second attempts, mapped back to the identifier the application holds.
    attempts: BTreeMap<TaskId, TaskId>,
    /// The screen the runtime is believed to be showing.
    ///
    /// A screen identical to the one already on the panel is dropped here
    /// rather than sent. Gutenbird rebuilt and re-sent its whole reading
    /// screen on every 256 KB chunk of a download; the frame planner then
    /// found no changed pixel and did nothing, but the screen had already been
    /// laid out, encoded, written and decoded to discover that. Forgotten
    /// whenever the panel could have gone elsewhere -- a launch, or being sent
    /// to the background -- because after that the runtime is showing somebody
    /// else's screen and skipping the resend would leave it there.
    displayed: Option<Screen>,
}

impl<A: KoboApp> AppRunner<A> {
    #[must_use]
    pub fn new(app: A) -> Self {
        // The same typeface the runtime lays out with, installed here as well
        // as on the socket path, because an application's own tests are where
        // wrapping, pagination and one-line labels are actually asserted. Left
        // to the built-in bitmap they would be asserted against a fixed-width
        // uppercase fallback nothing ever draws with.
        #[cfg(feature = "text")]
        let _ = kobo_text::install(DisplayMetrics::default());
        Self {
            app,
            metrics: DisplayMetrics::default(),
            started: false,
            pending: VecDeque::new(),
            pending_stores: 0,
            next_task: 0,
            in_flight: 0,
            settled: false,
            retrying: BTreeMap::new(),
            napping: BTreeMap::new(),
            attempts: BTreeMap::new(),
            displayed: None,
        }
    }

    /// Runs an application against a specific panel.
    ///
    /// [`AppRunner::new`] assumes the default Clara BW metrics, which is right
    /// for a test and wrong for the device: the runtime states which panel it
    /// owns during the handshake.
    #[must_use]
    pub fn with_metrics(app: A, metrics: DisplayMetrics) -> Self {
        #[cfg(feature = "text")]
        let _ = kobo_text::install(metrics);
        Self {
            metrics,
            ..Self::new(app)
        }
    }

    #[must_use]
    pub const fn app(&self) -> &A {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// A context measuring against the same panel the application ran on.
    ///
    /// For tests that need to ask what fits: a test computing its own widths
    /// is testing its own arithmetic, and the whole point of `one_line_row`
    /// and `clamped_row` is that there is exactly one measure.
    #[must_use]
    pub fn context(&self) -> Context {
        Context {
            commands: Vec::new(),
            next_task: self.next_task,
            in_flight: self.in_flight,
            metrics: self.metrics,
            retrying: Vec::new(),
        }
    }

    pub fn start(&mut self) -> Vec<Command> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        self.dispatch(KoboApp::on_start)
    }

    pub fn action(&mut self, action: ActionId) -> Vec<Command> {
        // Answered here rather than passed on, so that no application has to
        // know the name of the settings application or that it is the thing
        // that owns the radio. An application that never offers the control
        // never sees the action, because nothing on its screen raises it.
        if action == action_id(JOIN_WIFI) {
            return self.dispatch(|_, context| context.launch(NETWORK_SETTINGS_APP));
        }
        self.dispatch(|app, context| app.on_action(context, action))
    }

    pub fn text_hold(&mut self, action: ActionId, hit: TextHit) -> Vec<Command> {
        self.dispatch(|app, context| app.on_text_hold(context, action, hit))
    }

    pub fn resume(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_resume)
    }

    pub fn suspend(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_suspend)
    }

    pub fn scheduled_wake(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_scheduled_wake)
    }

    pub fn exit(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_exit)
    }

    /// Delivers one hall-sensor change.
    ///
    /// Unlike a device answer this is not matched to anything outstanding,
    /// because nothing asked for it.
    pub fn cover_changed(&mut self, magnet_present: bool) -> Vec<Command> {
        self.dispatch(|app, context| app.on_cover_change(context, magnet_present))
    }

    /// Delivers one page-turn key press to the application.
    ///
    /// Unsolicited, like [`Self::cover_changed`]: nothing asked for it, the
    /// reader pressed a button.
    pub fn page_turn(&mut self, forward: bool) -> Vec<Command> {
        self.dispatch(|app, context| app.on_page_turn(context, forward))
    }

    /// Delivers one device answer, matched to the request that produced it.
    ///
    /// Answers arrive in request order on a single ordered stream. An answer
    /// with nothing outstanding is ignored rather than mismatched.
    pub fn device_result(&mut self, result: DeviceResult) -> Vec<Command> {
        let Some(request) = self.pending.pop_front() else {
            return Vec::new();
        };
        self.dispatch(|app, context| app.on_device_result(context, request, result))
    }

    /// Delivers the outcome of one task.
    ///
    /// Reporting back frees the slot the task occupied, so an application that
    /// keeps starting work can keep starting it, while one that never hears
    /// back stays capped.
    pub fn task_outcome(&mut self, task: TaskId, outcome: TaskOutcome) -> Vec<Command> {
        // A nap between two attempts finishing is not news for the
        // application, which never learned there was one.
        if let Some((waiting_on, work)) = self.napping.remove(&task) {
            if matches!(outcome, TaskOutcome::Cancelled) {
                self.settled = true;
                return self.dispatch(|app, context| {
                    app.on_task(context, waiting_on, TaskOutcome::Cancelled);
                });
            }
            let (again, command) = self.hand_over(work);
            self.attempts.insert(again, waiting_on);
            return vec![command];
        }
        // A second attempt reports under the identifier the application is
        // holding, never the one this runner invented for it.
        let task = self.attempts.remove(&task).unwrap_or(task);
        if let TaskOutcome::Failed(error) = outcome {
            if error.worth_retrying() {
                if let Some(work) = self.retrying.remove(&task) {
                    let (nap, command) = self.hand_over(Task::Sleep {
                        seconds: RETRY_DELAY_SECONDS,
                    });
                    self.napping.insert(nap, (task, work));
                    return vec![command];
                }
            }
        }
        self.retrying.remove(&task);
        self.settled = true;
        self.dispatch(|app, context| app.on_task(context, task, outcome))
    }

    /// Starts work the application did not ask for directly.
    ///
    /// One task has just settled and one is starting in its place, so the
    /// count of tasks in flight does not move and `settled` stays false: there
    /// is no callback coming that would settle it.
    fn hand_over(&mut self, work: Task) -> (TaskId, Command) {
        self.next_task = self.next_task.saturating_add(1);
        let task = TaskId(self.next_task);
        (task, Command::Spawn { task, work })
    }

    /// The identifier the runtime knows a task by, which is not the one the
    /// application holds while a second attempt is in the air.
    fn live_attempt(&self, held: TaskId) -> Option<TaskId> {
        self.attempts
            .iter()
            .find_map(|(actual, waiting_on)| (*waiting_on == held).then_some(*actual))
            .or_else(|| {
                self.napping
                    .iter()
                    .find_map(|(nap, (waiting_on, _))| (*waiting_on == held).then_some(*nap))
            })
    }

    /// Tells the application it gained or lost the panel.
    pub fn lifecycle(&mut self, state: Lifecycle) -> Vec<Command> {
        match state {
            Lifecycle::Foreground => self.dispatch(KoboApp::on_foreground),
            Lifecycle::Background => {
                // Somebody else has the panel now, so the next screen this
                // application builds must be sent even if it is the same one
                // it was showing when it lost it.
                self.displayed = None;
                self.dispatch(KoboApp::on_background)
            }
        }
    }

    /// Forgets what the panel is believed to be showing.
    ///
    /// For a host that repaints from somewhere other than this application, so
    /// the next identical screen is sent rather than skipped.
    pub fn forget_displayed(&mut self) {
        self.displayed = None;
    }

    /// Delivers one store answer.
    pub fn store_result(&mut self, result: StoreResult) -> Vec<Command> {
        self.pending_stores = self.pending_stores.saturating_sub(1);
        self.dispatch(|app, context| app.on_store(context, result))
    }

    /// Delivers one terminal event.
    pub fn shell_event(&mut self, event: ShellEvent) -> Vec<Command> {
        self.dispatch(|app, context| app.on_shell_event(context, event))
    }

    /// The device requests still awaiting an answer, oldest first.
    #[must_use]
    pub fn outstanding_requests(&self) -> usize {
        self.pending.len()
    }

    /// Every answer the runtime still owes this application.
    ///
    /// A harness that leaves while an answer is in flight closes the socket
    /// under a runtime that is about to write to it, which is a broken pipe on
    /// the runtime rather than the clean shutdown it looks like from here.
    #[must_use]
    pub fn outstanding_answers(&self) -> usize {
        self.pending.len() + self.pending_stores
    }

    #[must_use]
    pub const fn tasks_in_flight(&self) -> usize {
        self.in_flight
    }

    fn dispatch(&mut self, callback: impl FnOnce(&mut A, &mut Context)) -> Vec<Command> {
        let mut context = Context {
            commands: Vec::new(),
            next_task: self.next_task,
            in_flight: self.in_flight,
            metrics: self.metrics,
            retrying: Vec::new(),
        };
        if std::mem::take(&mut self.settled) {
            context.settle();
        }
        let started = std::time::Instant::now();
        callback(&mut self.app, &mut context);
        let elapsed = started.elapsed();
        self.next_task = context.next_task;
        self.in_flight = context.in_flight;
        for (task, work) in std::mem::take(&mut context.retrying) {
            self.retrying.insert(task, work);
        }
        let mut commands = context.take_commands();
        // An application cancelling a task names it by the identifier it was
        // given, which is not what the runtime is holding once a second
        // attempt is in the air. Left untranslated the cancel would name
        // nothing and the attempt would run on.
        for command in &mut commands {
            if let Command::Cancel(held) = command {
                if let Some(actual) = self.live_attempt(*held) {
                    self.retrying.remove(held);
                    *command = Command::Cancel(actual);
                }
            }
        }
        commands.retain(|command| match command {
            Command::SetScreen(screen) => {
                if self.displayed.as_ref() == Some(screen) {
                    return false;
                }
                self.displayed = Some(screen.clone());
                true
            }
            // Anything that can hand the panel to somebody else invalidates
            // what we believe is on it, and so does handing over pixels. A
            // screen refers to a picture by handle, so filling that handle in
            // later changes what is on the panel without changing a byte of
            // the tree that describes it -- and the tree is all the
            // comparison above can see. A book of plates opened on an
            // illustration and showed an empty frame until something
            // unrelated happened to redraw the page, because every repaint
            // the pipeline asked for was thrown away here for being identical
            // to the one already displayed.
            Command::Launch(_) | Command::Exit | Command::PutPicture { .. } => {
                self.displayed = None;
                true
            }
            _ => true,
        });
        for command in &commands {
            match command {
                Command::Device(request) => self.pending.push_back(request.clone()),
                Command::Store(_) => {
                    self.pending_stores = self.pending_stores.saturating_add(1);
                }
                _ => {}
            }
        }
        // An application that blocks here has already held the only thread that
        // can repaint the screen or read a touch. The overrun cannot be
        // prevented from inside the callback, so it is reported instead, and
        // the host runtime is expected to act on a repeat offender.
        if elapsed > CALLBACK_DEADLINE {
            commands.insert(
                0,
                Command::Log {
                    level: LogLevel::Warn,
                    message: format!(
                        "a lifecycle callback ran for {} ms, over the {} ms deadline; move this work to Context::spawn",
                        elapsed.as_millis(),
                        CALLBACK_DEADLINE.as_millis()
                    ),
                },
            );
        }
        commands
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientEvent {
    Action(ActionId),
    TextHold {
        action: ActionId,
        hit: TextHit,
    },
    Device(DeviceResult),
    Task {
        task: TaskId,
        outcome: TaskOutcome,
    },
    Store(StoreResult),
    Lifecycle(Lifecycle),
    Shell(ShellEvent),
    /// A magnet arrived at, or left, the hall sensor. Unsolicited.
    CoverChanged(bool),
    /// A physical page-turn key was pressed. Unsolicited; `true` is forward.
    PageTurn(bool),
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientError {
    Stream(StreamError),
    UnexpectedMessage,
    /// The runtime did not say where to connect, which means this binary was
    /// started by something other than the runtime.
    MissingSocket,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stream(error) => write!(formatter, "{error}"),
            Self::UnexpectedMessage => formatter.write_str("unexpected daemon message"),
            Self::MissingSocket => formatter.write_str(
                "KOBO_SOCKET is not set; a Kobo application is started by the runtime, not directly",
            ),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<StreamError> for ClientError {
    fn from(error: StreamError) -> Self {
        Self::Stream(error)
    }
}

/// Synchronous client for one managed Kobo application.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    next_request: u32,
    metrics: DisplayMetrics,
}

impl Client {
    /// Connects to `kobod` and completes the protocol handshake.
    ///
    /// # Errors
    ///
    /// Returns a stream error when the socket cannot be opened or the handshake
    /// cannot be exchanged, and `UnexpectedMessage` for a non-welcome response.
    pub fn connect(path: impl AsRef<Path>, app_name: &str) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(StreamError::from)?;
        Self::from_stream(stream, app_name)
    }

    /// Completes a handshake over an already connected private stream.
    ///
    /// # Errors
    ///
    /// Returns a protocol or stream error, or `UnexpectedMessage` when the peer
    /// does not identify itself as `kobod`.
    pub fn from_stream(mut stream: UnixStream, app_name: &str) -> Result<Self, ClientError> {
        kobo_protocol::write_to(
            &mut stream,
            &Frame {
                version: kobo_protocol::VERSION,
                request_id: 1,
                message: Message::Hello {
                    name: app_name.to_owned(),
                },
            },
        )?;
        let response = kobo_protocol::read_from(&mut stream)?;
        let Message::Welcome {
            width,
            height,
            pixels_per_inch,
            text_scale,
        } = response.message
        else {
            return Err(ClientError::UnexpectedMessage);
        };
        Ok(Self {
            stream,
            next_request: 2,
            metrics: DisplayMetrics {
                width: i32::from(width),
                height: i32::from(height),
                pixels_per_inch: i32::from(pixels_per_inch),
                text_scale,
            },
        })
    }

    /// The panel this application is running on.
    ///
    /// Learned from the runtime rather than assumed, so an application that
    /// measures text measures it for the panel it is actually drawing to.
    #[must_use]
    pub const fn metrics(&self) -> DisplayMetrics {
        self.metrics
    }

    /// Sends all commands produced by an application callback.
    ///
    /// # Errors
    ///
    /// Returns a stream or protocol error if any command cannot be delivered.
    pub fn send_commands(
        &mut self,
        commands: impl IntoIterator<Item = Command>,
    ) -> Result<(), ClientError> {
        for command in commands {
            let command = match command {
                Command::PutPicture {
                    handle,
                    width,
                    height,
                    grey,
                } => {
                    if grey.len() <= MAX_INLINE_PICTURE_BYTES {
                        self.send(Message::PutPicture {
                            handle,
                            width,
                            height,
                            grey,
                        })?;
                    } else {
                        self.send(Message::BeginPicture {
                            handle,
                            width,
                            height,
                        })?;
                        for (index, chunk) in grey.chunks(MAX_PICTURE_CHUNK_BYTES).enumerate() {
                            let offset = index
                                .checked_mul(MAX_PICTURE_CHUNK_BYTES)
                                .and_then(|offset| u32::try_from(offset).ok())
                                .ok_or(ClientError::Stream(StreamError::Protocol(
                                    kobo_protocol::ProtocolError::FrameTooLarge,
                                )))?;
                            self.send(Message::PictureChunk {
                                handle,
                                offset,
                                grey: chunk.to_vec(),
                            })?;
                        }
                        self.send(Message::CommitPicture { handle })?;
                    }
                    continue;
                }
                other => other,
            };
            let message = match command {
                Command::SetScreen(screen) => Message::SetScreen(screen),
                Command::Log { level, message } => Message::Log { level, message },
                Command::Device(request) => Message::DeviceRequest(request),
                Command::Spawn { task, work } => Message::Spawn { task, work },
                Command::Cancel(task) => Message::Cancel { task },
                Command::Store(request) => Message::StoreRequest(request),
                Command::Shell(request) => Message::ShellRequest(request),
                Command::Exit => Message::Exit,
                Command::Launch(name) => Message::Launch { name },
                Command::PutPicture { .. } => unreachable!("handled above"),
                Command::DropPicture(handle) => Message::DropPicture { handle },
                Command::PutFont {
                    handle,
                    name,
                    bytes,
                } => Message::PutFont {
                    handle,
                    name,
                    bytes,
                },
                Command::DropFont(handle) => Message::DropFont { handle },
            };
            self.send(message)?;
        }
        Ok(())
    }

    /// Waits for the next user action or daemon exit request.
    ///
    /// # Errors
    ///
    /// Returns a stream/protocol error or `UnexpectedMessage` for a message that
    /// is not an application event.
    pub fn next_event(&mut self) -> Result<ClientEvent, ClientError> {
        match kobo_protocol::read_from(&mut self.stream)?.message {
            Message::Action { action } => Ok(ClientEvent::Action(action)),
            Message::TextHold {
                action,
                context,
                start,
                end,
            } => Ok(ClientEvent::TextHold {
                action,
                hit: TextHit {
                    context,
                    start,
                    end,
                },
            }),
            Message::DeviceResult(result) => Ok(ClientEvent::Device(result)),
            Message::TaskOutcome { task, outcome } => Ok(ClientEvent::Task { task, outcome }),
            Message::StoreResult(result) => Ok(ClientEvent::Store(result)),
            Message::Lifecycle(state) => Ok(ClientEvent::Lifecycle(state)),
            Message::ShellEvent(event) => Ok(ClientEvent::Shell(event)),
            Message::CoverChanged { magnet_present } => {
                Ok(ClientEvent::CoverChanged(magnet_present))
            }
            Message::PageTurn { forward } => Ok(ClientEvent::PageTurn(forward)),
            Message::Exit => Ok(ClientEvent::Exit),
            _ => Err(ClientError::UnexpectedMessage),
        }
    }

    fn send(&mut self, message: Message) -> Result<(), ClientError> {
        let request_id = self.next_request;
        self.next_request = self.next_request.wrapping_add(1).max(2);
        kobo_protocol::write_to(
            &mut self.stream,
            &Frame {
                version: kobo_protocol::VERSION,
                request_id,
                message,
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn an_update_that_could_not_possibly_verify_is_refused_before_the_wire() {
        struct Updater;
        impl KoboApp for Updater {
            fn on_start(&mut self, context: &mut Context) {
                let good_digest = "a".repeat(64);
                assert!(
                    !context
                        .device()
                        .update("http://plain.example", &good_digest),
                    "an update must not travel over plain HTTP"
                );
                assert!(
                    !context.device().update("https://good.example", "DEADBEEF"),
                    "a string that is not a digest can never match a download"
                );
                assert!(context
                    .device()
                    .update("https://good.example", &good_digest));
            }
            fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        }
        let queued = AppRunner::new(Updater)
            .start()
            .into_iter()
            .filter(|command| matches!(command, Command::Device(DeviceRequest::Update { .. })))
            .count();
        assert_eq!(queued, 1, "only the well-formed request may be queued");
    }

    #[test]
    fn app_store_link_helpers_queue_device_requests() {
        let mut context = Context::default();
        {
            let mut store = context.store();
            store.begin_link();
            store.read_link();
            store.poll_link();
            store.disconnect_link();
        }
        assert_eq!(
            context.take_commands(),
            vec![
                Command::Device(DeviceRequest::BeginAppLink),
                Command::Device(DeviceRequest::ReadAppLink),
                Command::Device(DeviceRequest::PollAppLink),
                Command::Device(DeviceRequest::DisconnectAppLink),
            ]
        );
    }

    #[test]
    fn a_screen_identical_to_the_one_showing_is_not_sent_again() {
        #[derive(Default)]
        struct Counter {
            received: u64,
        }
        impl KoboApp for Counter {
            fn on_start(&mut self, context: &mut Context) {
                self.paint(context);
            }
            fn on_action(&mut self, context: &mut Context, _action: ActionId) {
                self.paint(context);
            }
        }
        impl Counter {
            fn paint(&self, context: &mut Context) {
                context.set_screen(
                    ScreenBuilder::new("counter")
                        .transfer("Downloading", self.received, Some(11_534_336))
                        .build(),
                );
            }
        }

        let mut runner = AppRunner::new(Counter::default());
        let sent = |commands: &[Command]| {
            commands
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count()
        };
        assert_eq!(sent(&runner.start()), 1, "the first screen always goes");
        assert_eq!(
            sent(&runner.action(ActionId(1))),
            0,
            "an unchanged screen costs the wire, the renderer and the panel nothing"
        );
        runner.app_mut().received = 4_404_019;
        assert_eq!(
            sent(&runner.action(ActionId(1))),
            1,
            "a transfer that has moved must still repaint"
        );
    }

    /// A picture arriving after the screen that refers to it still repaints.
    ///
    /// A screen names a picture by handle, so filling that handle in later
    /// changes the panel without changing the tree. On the reader this was a
    /// book of plates opening on an illustration and drawing an empty frame:
    /// the pipeline decoded the plate, handed over the pixels and asked for a
    /// repaint, and the repaint was thrown away for being identical to the
    /// screen already displayed. The frame stayed empty until a page turn
    /// happened to change the tree for some other reason.
    #[test]
    fn pixels_handed_over_after_the_screen_that_names_them_still_reach_the_panel() {
        #[derive(Default)]
        struct Plate {
            filled: bool,
        }
        impl KoboApp for Plate {
            fn on_start(&mut self, context: &mut Context) {
                Self::paint(context);
            }
            fn on_action(&mut self, context: &mut Context, _action: ActionId) {
                if !self.filled {
                    self.filled = true;
                    let _ = context.put_picture(PictureHandle(1), 2, 2, vec![0, 1, 2, 3]);
                }
                Self::paint(context);
            }
        }
        impl Plate {
            fn paint(context: &mut Context) {
                // The same tree either way: what changed is behind the handle.
                context.set_screen(ScreenBuilder::new("plate").text("A plate.").build());
            }
        }

        let mut runner = AppRunner::new(Plate::default());
        let sent = |commands: &[Command]| {
            commands
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count()
        };
        assert_eq!(sent(&runner.start()), 1, "the first screen always goes");
        assert_eq!(
            sent(&runner.action(ActionId(1))),
            1,
            "the plate was handed over and the page was never redrawn to show it"
        );
        assert_eq!(
            sent(&runner.action(ActionId(1))),
            0,
            "an unchanged screen with no new pixels still costs nothing"
        );
    }

    #[test]
    fn a_hero_puts_the_cover_beside_the_metadata_and_adds_no_node_kind() {
        let screen = ScreenBuilder::new("book")
            .hero(
                Some(TilePicture::new(PictureHandle(1), 300, 450)),
                40,
                "Moby Dick",
                Some("Herman Melville".to_owned()),
                [("Language", "English"), ("Downloads", "12,043")],
            )
            .build();
        assert!(
            matches!(screen.nodes.as_slice(), [Node::Band { slots, .. }] if slots.len() == 2),
            "a hero is a band, not a node of its own: {:?}",
            screen.nodes
        );
    }

    #[test]
    fn a_hero_without_a_cover_gives_the_metadata_the_whole_width() {
        let screen = ScreenBuilder::new("book")
            .hero(None, 40, "Moby Dick", None, [("Language", "English")])
            .build();
        assert!(
            !screen
                .nodes
                .iter()
                .any(|node| matches!(node, Node::Band { .. })),
            "nothing should sit beside a cover that never arrived: {:?}",
            screen.nodes
        );
    }

    #[test]
    fn two_secondary_actions_share_one_line() {
        // Stacked, each took the full width of the panel to say one word.
        let screen = ScreenBuilder::new("todo")
            .buttons([("add", "Add"), ("clear", "Clear finished")])
            .build();
        let Some(Node::Band { slots, .. }) = screen.nodes.first() else {
            panic!("a pair of actions is a band: {:?}", screen.nodes);
        };
        assert_eq!(slots.len(), 2);
        assert!(
            slots
                .iter()
                .all(|slot| matches!(slot.nodes.as_slice(), [Node::Button { .. }])),
            "a slot holds one button and nothing else"
        );
    }

    #[test]
    fn one_action_beside_nothing_is_just_a_button() {
        // A band of one costs a node and reads the same.
        let screen = ScreenBuilder::new("todo").buttons([("add", "Add")]).build();
        assert!(
            matches!(screen.nodes.as_slice(), [Node::Button { .. }]),
            "{:?}",
            screen.nodes
        );
    }

    #[test]
    fn paired_actions_are_still_separately_addressable() {
        // The failure this guards: a band that registered one action for the
        // whole row would make the second button do the first one's job.
        let screen = ScreenBuilder::new("todo")
            .buttons([("add", "Add"), ("clear", "Clear finished")])
            .build();
        let Some(Node::Band { slots, .. }) = screen.nodes.first() else {
            panic!("a pair of actions is a band");
        };
        let actions: Vec<ActionId> = slots
            .iter()
            .filter_map(|slot| match slot.nodes.as_slice() {
                [Node::Button { action, .. }] => Some(*action),
                _ => None,
            })
            .collect();
        assert_eq!(actions.len(), 2);
        assert_ne!(actions[0], actions[1], "both buttons sent the same action");
    }

    #[test]
    fn three_dots_open_a_menu_that_a_tap_anywhere_else_closes() {
        let screen = ScreenBuilder::new("story")
            .top_bar("Moby Dick")
            .top_bar_overflow("more", true, [("save", "Save"), ("share", "Copy the link")])
            .build();
        let bar = screen.top_bar.expect("a top bar");
        assert_eq!(bar.actions.len(), 1, "the bar does not grow, the menu does");
        assert_eq!(bar.actions[0].glyph, Some(kobo_ui::Glyph::More));
        let overlay = screen.overlay.expect("a popover");
        assert!(
            matches!(overlay.kind, kobo_ui::OverlayKind::Popover { anchor } if anchor == bar.actions[0].action),
            "the menu must hang off the dots it came out of"
        );
        assert!(
            overlay.dismissed_by_a_miss(),
            "a menu that has to be told to close is a menu that gets left open"
        );
        assert_eq!(overlay.nodes.len(), 2);
    }

    /// The dots are drawn whether or not the menu is showing.
    ///
    /// Otherwise the bar has two items on one screen and three on the next,
    /// and every verb in it moves sideways the moment a menu opens.
    #[test]
    fn a_closed_menu_still_leaves_its_dots_in_the_bar() {
        let screen = ScreenBuilder::new("story")
            .top_bar("Moby Dick")
            .top_bar_overflow("more", false, [("save", "Save")])
            .build();
        let bar = screen.top_bar.expect("a top bar");
        assert_eq!(bar.actions.len(), 1);
        assert_eq!(bar.actions[0].glyph, Some(kobo_ui::Glyph::More));
        assert!(screen.overlay.is_none(), "a closed menu was drawn open");
    }

    #[test]
    fn three_dots_with_nothing_under_them_are_not_drawn() {
        let screen = ScreenBuilder::new("story")
            .top_bar("Moby Dick")
            .top_bar_overflow("more", true, Vec::<(String, String)>::new())
            .build();
        assert!(screen.top_bar.expect("a top bar").actions.is_empty());
        assert!(screen.overlay.is_none());
    }

    /// The panel has one bottom band, and losing a bar to it should say so.
    ///
    /// The failure this catches: a screen builds an action bar, a shared
    /// helper then appends the application's navigation, and the verbs vanish
    /// with nothing anywhere reporting it.
    #[test]
    fn a_second_bottom_bar_is_reported_rather_than_swallowed() {
        let quiet = ScreenBuilder::new("one").action_bar([("save", "Save")]);
        assert!(quiet.warnings().is_empty(), "one bar warned about nothing");

        let two = ScreenBuilder::new("two")
            .action_bar([("save", "Save")])
            .nav_bar(0, [("a", "A"), ("b", "B")]);
        assert!(
            two.warnings().iter().any(|issue| matches!(
                issue.kind,
                LayoutIssueKind::CollectionTruncated {
                    collection: "bottom bar",
                    ..
                }
            )),
            "the action bar was replaced in silence: {:?}",
            two.warnings()
        );
    }

    /// A bare context, for exercising the request builders directly.
    fn context() -> Context {
        Context {
            commands: Vec::new(),
            next_task: 1,
            in_flight: 0,
            metrics: DisplayMetrics::default(),
            retrying: Vec::new(),
        }
    }

    #[test]
    fn a_grid_fills_every_row_the_panel_can_draw() {
        // The launcher holds twelve applications and a Clara draws three rows
        // of three. It shipped showing six, with four hundred pixels of paper
        // under them and the other six on a second page, for two reasons that
        // both came down to reserving room nobody was going to use: the row
        // gap was measured with the wider column gutter, and the page
        // position's strip was subtracted from a screen that draws no page
        // position.
        let context = context();
        let pages = context.paginate_tiles(12, TileShape::Square, true);
        assert_eq!(
            pages[0].len(),
            9,
            "a Clara's grid came back holding {} of its nine cells",
            pages[0].len()
        );

        // And the promise is kept: every cell of the page is drawn.
        let mut screen = ScreenBuilder::new("launcher").top_bar("Cobalt 1 of 2");
        screen = screen.tiles(
            pages[0]
                .iter()
                .map(|index| (format!("open-{index}"), format!("App {index}"), Glyph::App)),
        );
        let drawn = screen
            .build()
            .layout_with(&context.metrics, &Chrome::measuring(true))
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, kobo_ui::LayoutKind::Tile(..)))
            .count();
        assert_eq!(drawn, pages[0].len(), "the grid dropped what it was given");
    }

    /// A tick is not the application's answer, and a stopped clock stops.
    #[test]
    fn a_heartbeat_re_arms_itself_until_it_is_stopped() {
        let mut context = context();
        let mut clock = Heartbeat::every(5);
        assert!(!clock.is_running());
        clock.start(&mut context);
        let first = clock.task.expect("the clock armed a nap");
        assert!(clock.waited_words().is_empty(), "nothing has been waited");

        // Somebody else's task is left alone.
        assert!(!clock.on_task(&mut context, TaskId(first.0 + 99), &TaskOutcome::Cancelled));

        assert!(clock.on_task(&mut context, first, &TaskOutcome::Completed(Vec::new())));
        assert_eq!(clock.waited(), Duration::from_secs(5));
        let second = clock.task.expect("the clock re-armed");
        assert_ne!(second, first);
        assert!(clock.on_task(&mut context, second, &TaskOutcome::Completed(Vec::new())));
        assert_eq!(clock.waited_words(), "10 seconds so far");

        clock.stop(&mut context);
        assert!(!clock.is_running());
        assert_eq!(clock.waited(), Duration::ZERO);
        assert!(context
            .commands
            .iter()
            .any(|command| matches!(command, Command::Cancel(_))));
    }

    /// A cancelled nap is the runtime agreeing to stop, not a tick.
    #[test]
    fn a_cancelled_nap_does_not_re_arm_the_clock() {
        let mut context = context();
        let mut clock = Heartbeat::every(5);
        clock.start(&mut context);
        let task = clock.task.expect("the clock armed a nap");
        assert!(clock.on_task(&mut context, task, &TaskOutcome::Cancelled));
        assert!(!clock.is_running());
    }

    /// The default is the cadence, not the zero value.
    ///
    /// A clock left at `u32::default()` asks for a zero second sleep, which
    /// the runtime completes at once, which re-arms it: four hundred and fifty
    /// naps went out in the half minute this was live, and the application
    /// that held the clock never showed a single tick.
    #[test]
    fn a_missing_key_can_be_named() {
        let missing = Failure::of(TaskError::NoCredential);
        let said = missing.naming("elevenlabs");
        assert!(said.contains("called elevenlabs"), "{said}");
        assert!(said.contains("kobo secret set elevenlabs"), "{said}");
        assert!(!said.contains("that service"), "{said}");

        // Naming a key is meaningless for a failure that had nothing to do
        // with one, so the sentence is left exactly as it was.
        let slow = Failure::of(TaskError::TimedOut);
        assert_eq!(slow.naming("elevenlabs"), slow.advice);
    }

    #[test]
    fn a_default_heartbeat_naps_for_a_sensible_time() {
        let mut context = context();
        let mut clock = Heartbeat::default();
        clock.start(&mut context);
        let napped = context
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn {
                    work: Task::Sleep { seconds },
                    ..
                } => Some(*seconds),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(napped, vec![DEFAULT_HEARTBEAT_SECONDS]);
        const { assert!(DEFAULT_HEARTBEAT_SECONDS > 0) };
    }

    #[test]
    fn a_long_wait_is_spelled_in_minutes() {
        let mut clock = Heartbeat::every(5);
        clock.waited = 60;
        assert_eq!(clock.waited_words(), "1 min so far");
        clock.waited = 135;
        assert_eq!(clock.waited_words(), "2 min 15 s so far");
    }

    /// Runs a transfer against the real policy shelf until it stops.
    ///
    /// Not a stub. A chunking bug is an agreement between two sides, and a
    /// fake that agrees with the code under test proves only that the code
    /// agrees with itself.
    fn drive(
        shelf: &kobo_policy::shelf::Shelf,
        mut step: impl FnMut(&mut Context, Option<&StoreResult>) -> ShelfProgress,
    ) -> (ShelfProgress, usize) {
        let mut context = context();
        let mut progress = step(&mut context, None);
        let mut trips = 0;
        while matches!(progress, ShelfProgress::Moving { .. }) || trips == 0 {
            let Some(Command::Store(request)) = context.take_commands().pop() else {
                break;
            };
            trips += 1;
            assert!(trips < 10_000, "a transfer did not terminate");
            let result = shelf.handle(&request).expect("a shelf request");
            progress = step(&mut context, Some(&result));
            if !matches!(progress, ShelfProgress::Moving { .. }) {
                break;
            }
        }
        (progress, trips)
    }

    fn temporary_shelf() -> kobo_policy::shelf::Shelf {
        let root = std::env::temp_dir().join(format!(
            "kobo-sdk-shelf-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let _ignored = std::fs::remove_dir_all(&root);
        kobo_policy::shelf::Shelf::new(root)
    }

    #[test]
    fn a_blob_larger_than_a_frame_survives_the_whole_round_trip() {
        // The case the shelf exists for: bigger than one message, so it can
        // only work if the cutting and the rejoining agree.
        let shelf = temporary_shelf();
        let book: Vec<u8> = (0..600_000u32).map(|index| (index % 251) as u8).collect();

        let mut upload = ShelfUpload::new("book.txt", book.clone());
        let (progress, trips) = drive(&shelf, |context, result| match result {
            None => {
                upload.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => upload.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Done);
        assert_eq!(trips, 3, "600 kB took an unexpected number of round trips");

        let mut download = ShelfDownload::new("book.txt");
        let (progress, _) = drive(&shelf, |context, result| match result {
            None => {
                download.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => download.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Done);
        assert_eq!(download.take(), book, "the book came back different");
    }

    #[test]
    fn an_empty_blob_still_finishes() {
        // Zero bytes is a real answer -- an empty file, a book that failed to
        // download -- and a loop written around "send until empty" hangs here.
        let shelf = temporary_shelf();
        let mut upload = ShelfUpload::new("empty.txt", Vec::new());
        let (progress, trips) = drive(&shelf, |context, result| match result {
            None => {
                upload.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => upload.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Done);
        assert_eq!(trips, 1);

        let mut download = ShelfDownload::new("empty.txt");
        let (progress, _) = drive(&shelf, |context, result| match result {
            None => {
                download.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => download.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Done);
        assert!(download.take().is_empty());
    }

    #[test]
    fn a_restarted_upload_replaces_what_the_first_attempt_left() {
        let shelf = temporary_shelf();
        let mut abandoned = ShelfUpload::new("book.txt", vec![b'x'; 300_000]);
        let mut context = context();
        abandoned.start(&mut context);
        if let Some(Command::Store(request)) = context.take_commands().pop() {
            let _ignored = shelf.handle(&request);
        }

        let mut upload = ShelfUpload::new("book.txt", b"the real one".to_vec());
        let (progress, _) = drive(&shelf, |context, result| match result {
            None => {
                upload.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => upload.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Done);

        let mut download = ShelfDownload::new("book.txt");
        let (progress, _) = drive(&shelf, |context, result| match result {
            None => {
                download.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => download.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Done);
        assert_eq!(download.take(), b"the real one");
    }

    #[test]
    fn a_download_of_something_that_is_not_there_fails_rather_than_returning_nothing() {
        // An empty Vec and a missing book must not look the same, or an
        // application shows a reader full of blank pages instead of an error.
        let shelf = temporary_shelf();
        let mut download = ShelfDownload::new("absent.txt");
        let (progress, _) = drive(&shelf, |context, result| match result {
            None => {
                download.start(context);
                ShelfProgress::Moving { done: 0, total: 0 }
            }
            Some(result) => download.advance(context, result),
        });
        assert_eq!(progress, ShelfProgress::Failed(StoreError::Missing));
    }

    #[test]
    fn a_transfer_that_has_received_nothing_reports_no_amount() {
        let nodes = |received, total| match ScreenBuilder::new("t")
            .transfer("Downloading", received, total)
            .build()
            .nodes
            .first()
            .expect("the transfer was built")
        {
            Node::Activity { transferred, .. } => *transferred,
            other => panic!("a transfer is an activity, not {other:?}"),
        };
        // Nothing has arrived and nothing is expected: there is no amount, so
        // none is claimed. "0 B" reads as a download that is going wrong.
        assert_eq!(nodes(0, None), None);
        // Once bytes are in, or once a total is known, the report is real.
        assert_eq!(nodes(512, None), Some((512, None)));
        assert_eq!(nodes(0, Some(2048)), Some((0, Some(2048))));
    }

    #[test]
    fn a_transfer_ignores_an_answer_that_is_not_its_own() {
        let mut context = context();
        let mut upload = ShelfUpload::new("mine.txt", b"x".to_vec());
        assert_eq!(
            upload.advance(
                &mut context,
                &StoreResult::ShelfWritten {
                    name: "theirs.txt".into(),
                    size: 1,
                }
            ),
            ShelfProgress::Elsewhere
        );
        assert_eq!(
            upload.advance(&mut context, &StoreResult::Keys(Vec::new())),
            ShelfProgress::Elsewhere
        );
        assert!(
            context.take_commands().is_empty(),
            "a transfer acted on somebody else's answer"
        );
    }

    #[test]
    fn a_download_refuses_a_piece_that_does_not_join_on() {
        // Splicing at the wrong offset yields a file that parses and is not
        // the file, which is the failure hardest to notice.
        let mut context = context();
        let mut download = ShelfDownload::new("book.txt");
        assert_eq!(
            download.advance(
                &mut context,
                &StoreResult::ShelfRead {
                    name: "book.txt".into(),
                    offset: 4096,
                    bytes: b"middle".to_vec(),
                    size: 9000,
                }
            ),
            ShelfProgress::Failed(StoreError::Missing)
        );
    }

    #[test]
    fn a_download_stops_at_its_ceiling_instead_of_filling_memory() {
        let mut context = context();
        let mut download = ShelfDownload::new("huge.txt").at_most(1024);
        assert_eq!(
            download.advance(
                &mut context,
                &StoreResult::ShelfRead {
                    name: "huge.txt".into(),
                    offset: 0,
                    bytes: vec![0; 16],
                    size: 40_000_000,
                }
            ),
            ShelfProgress::Failed(StoreError::TooFull)
        );
        assert!(context.take_commands().is_empty());
    }

    struct Example;

    impl KoboApp for Example {
        fn on_start(&mut self, context: &mut Context) {
            context.set_screen(Screen::new(
                1,
                vec![Node::Button {
                    id: NodeId(1),
                    action: ActionId(1),
                    label: "Tap".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                }],
            ));
        }

        fn on_action(&mut self, context: &mut Context, action: ActionId) {
            context.log(LogLevel::Info, format!("action {}", action.0));
        }
    }

    struct Tofu;

    impl KoboApp for Tofu {
        fn on_start(&mut self, context: &mut Context) {
            // An ideograph, and not a tick: neither face on the device carries
            // one, whereas the tick this used to say has been drawn from the
            // grid face ever since the interface face got somewhere to fall
            // back to. A label that comes out fine is not a label worth
            // failing a build over.
            context.set_screen(
                ScreenBuilder::new("tofu")
                    .button("ok", "Chosen \u{4e2d}")
                    .build(),
            );
        }

        fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
    }

    /// A character the face has no glyph for is an empty box on the panel, and
    /// the only place that is cheap to find out is here.
    #[cfg(all(debug_assertions, feature = "text"))]
    #[test]
    #[should_panic(expected = "which the installed face cannot draw")]
    fn a_screen_carrying_a_character_the_face_cannot_draw_fails_here() {
        AppRunner::new(Tofu).start();
    }

    #[test]
    fn an_unanswered_store_request_keeps_an_application_from_leaving() {
        struct Loader;

        impl KoboApp for Loader {
            fn on_start(&mut self, context: &mut Context) {
                context.store().load("items");
            }

            fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        }

        let mut runner = AppRunner::new(Loader);
        assert!(matches!(runner.start().as_slice(), [Command::Store(_)]));
        // Nothing is outstanding by the device's reckoning, which is exactly
        // why a harness that only counted those closed the socket under a
        // runtime still holding an answer for it.
        assert_eq!(runner.outstanding_requests(), 0);
        assert_eq!(runner.outstanding_answers(), 1);
        runner.store_result(StoreResult::Loaded {
            key: "items".into(),
            value: None,
        });
        assert_eq!(runner.outstanding_answers(), 0);
    }

    #[test]
    fn runner_collects_lifecycle_commands() {
        let mut runner = AppRunner::new(Example);
        assert!(matches!(runner.start().as_slice(), [Command::SetScreen(_)]));
        assert!(runner.start().is_empty());
        assert!(matches!(
            runner.action(ActionId(9)).as_slice(),
            [Command::Log { .. }]
        ));
    }

    #[test]
    fn builder_uses_stable_nodes_and_action_names() {
        let builder = ScreenBuilder::new("hello")
            .heading("Hello, Kobo")
            .text("A dependency-free app")
            .button("close", "Close");
        assert_eq!(builder.action("close"), Some(action_id("close")));
        let screen = builder.build();
        assert_eq!(screen.id, ScreenBuilder::new("hello").build().id);
        assert_eq!(
            screen.nodes.iter().map(Node::id).collect::<Vec<_>>(),
            vec![NodeId(1), NodeId(2), NodeId(3)]
        );
        assert!(matches!(
            screen.nodes.last(),
            Some(Node::Button { action, .. }) if *action == action_id("close")
        ));
    }

    #[test]
    fn navigator_keeps_a_root_and_supports_push_replace_and_reset() {
        let mut navigation = Navigator::new("home");
        assert_eq!(navigation.current(), &"home");
        assert!(!navigation.back());
        navigation.push("details");
        navigation.replace("confirmation");
        assert_eq!(navigation.depth(), 2);
        assert_eq!(navigation.current(), &"confirmation");
        assert!(navigation.back());
        assert_eq!(navigation.current(), &"home");
        navigation.reset("library");
        assert_eq!(navigation.current(), &"library");
        assert!(!navigation.can_go_back());
    }

    #[test]
    fn standard_states_and_confirmations_have_consistent_structure() {
        let state = ScreenBuilder::new("offline")
            .offline_state("Reconnect, then try again.")
            .button("retry", "Try again")
            .build();
        // A splash and nothing above it. A banner here would be a second,
        // vaguer report of the event the splash already names.
        assert!(matches!(state.nodes.first(), Some(Node::Splash { .. })));
        assert!(
            !state
                .nodes
                .iter()
                .any(|node| matches!(node, Node::Banner { .. })),
            "a whole-screen state reports once"
        );
        // A splash rather than a heading: the title is centred in the room
        // that is left, and the button chained after it still lands under it.
        assert!(state
            .nodes
            .iter()
            .any(|node| matches!(node, Node::Splash { title, .. } if title == "You're offline")));
        assert!(matches!(state.nodes.last(), Some(Node::Button { .. })));

        let confirmation = ScreenBuilder::new("delete")
            .confirmation(
                "Delete this note?",
                "This cannot be undone.",
                DialogAction::new("delete", "Delete"),
                DialogAction::new("cancel", "Cancel").disabled(true),
            )
            .build();
        let states = confirmation
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Button { state, .. } => Some(*state),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(states, vec![ControlState::Enabled, ControlState::Disabled]);
    }

    #[test]
    fn application_metadata_builds_launcher_tiles_with_image_fallbacks() {
        const APP: AppMetadata = AppMetadata::new(
            "notes",
            "Notes",
            "Write without distraction.",
            AppIcon::picture(TilePicture::new(PictureHandle(8), 128, 128), Glyph::Note),
        );
        let screen = ScreenBuilder::new("apps").apps([APP]).build();
        let Node::TileGrid { tiles, .. } = &screen.nodes[0] else {
            panic!("metadata did not produce a tile grid");
        };
        assert_eq!(tiles[0].action, action_id(APP.id));
        assert_eq!(tiles[0].label, APP.display_name);
        assert_eq!(tiles[0].glyph, Glyph::Note);
        assert_eq!(
            tiles[0].picture,
            Some(TilePicture::new(PictureHandle(8), 128, 128))
        );
    }

    #[test]
    fn contextual_tiles_register_primary_and_visible_secondary_actions() {
        let builder = ScreenBuilder::new("tiles").contextual_tiles(
            TileShape::Card,
            [("open", "Reader", Glyph::Reader, "details")],
        );
        assert_eq!(builder.action("open"), Some(action_id("open")));
        assert_eq!(builder.action("details"), Some(action_id("details")));
        let screen = builder.build();
        let Node::TileGrid { tiles, .. } = &screen.nodes[0] else {
            panic!("contextual tiles did not produce a tile grid");
        };
        assert_eq!(tiles[0].action, action_id("open"));
        assert_eq!(tiles[0].menu, Some(action_id("details")));
    }

    #[test]
    fn checked_build_reports_collection_items_it_had_to_drop() {
        let builder = ScreenBuilder::new("choice").choose(
            "Pick one",
            (0..=MAX_CHOICE_OPTIONS).map(|index| {
                let name = format!("option-{index}");
                (name, format!("Option {index}"))
            }),
        );
        assert!(builder.warnings().iter().any(|issue| matches!(
            issue.kind,
            LayoutIssueKind::CollectionTruncated {
                collection: "choice options",
                ..
            }
        )));
        assert!(builder.build_checked().is_err());
    }

    #[test]
    fn action_ids_are_name_deterministic() {
        assert_eq!(action_id("increment"), action_id("increment"));
        assert_ne!(action_id("increment"), action_id("close"));
    }

    #[test]
    fn client_handshake_and_screen_delivery() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().expect("socket pair");
        let daemon = thread::spawn(move || {
            let hello = kobo_protocol::read_from(&mut daemon_stream).expect("hello");
            assert!(matches!(hello.message, Message::Hello { .. }));
            kobo_protocol::write_to(
                &mut daemon_stream,
                &Frame {
                    version: kobo_protocol::VERSION,
                    request_id: hello.request_id,
                    message: Message::Welcome {
                        width: 1072,
                        height: 1448,
                        pixels_per_inch: 300,
                        text_scale: kobo_ui::TextScale::Default,
                    },
                },
            )
            .expect("welcome");
            let screen = kobo_protocol::read_from(&mut daemon_stream).expect("screen");
            assert!(matches!(screen.message, Message::SetScreen(_)));
        });
        let mut client = Client::from_stream(client_stream, "counter").expect("connect");
        assert_eq!(client.metrics(), kobo_ui::CLARA_BW_METRICS);
        client
            .send_commands([Command::SetScreen(
                ScreenBuilder::new("counter").heading("Counter").build(),
            )])
            .expect("send screen");
        daemon.join().expect("daemon");
    }

    #[test]
    fn client_transparently_chunks_a_full_width_picture() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().expect("socket pair");
        let daemon = thread::spawn(move || {
            let hello = kobo_protocol::read_from(&mut daemon_stream).expect("hello");
            kobo_protocol::write_to(
                &mut daemon_stream,
                &Frame {
                    version: kobo_protocol::VERSION,
                    request_id: hello.request_id,
                    message: Message::Welcome {
                        width: 1072,
                        height: 1448,
                        pixels_per_inch: 300,
                        text_scale: kobo_ui::TextScale::Default,
                    },
                },
            )
            .expect("welcome");

            assert!(matches!(
                kobo_protocol::read_from(&mut daemon_stream)
                    .expect("begin")
                    .message,
                Message::BeginPicture {
                    handle: PictureHandle(9),
                    width: 1072,
                    height: 1448
                }
            ));
            let expected = 1072_usize * 1448;
            let mut received = 0;
            while received < expected {
                let Message::PictureChunk {
                    handle,
                    offset,
                    grey,
                } = kobo_protocol::read_from(&mut daemon_stream)
                    .expect("chunk")
                    .message
                else {
                    panic!("expected a picture chunk");
                };
                assert_eq!(handle, PictureHandle(9));
                assert_eq!(usize::try_from(offset).expect("offset"), received);
                assert!(grey.len() <= MAX_PICTURE_CHUNK_BYTES);
                received += grey.len();
            }
            assert!(matches!(
                kobo_protocol::read_from(&mut daemon_stream)
                    .expect("commit")
                    .message,
                Message::CommitPicture {
                    handle: PictureHandle(9)
                }
            ));
        });
        let mut client = Client::from_stream(client_stream, "gallery").expect("connect");
        client
            .send_commands([Command::PutPicture {
                handle: PictureHandle(9),
                width: 1072,
                height: 1448,
                grey: vec![127; 1072 * 1448],
            }])
            .expect("upload");
        daemon.join().expect("daemon");
    }
}

#[cfg(test)]
mod task_tests {
    use super::*;

    #[derive(Default)]
    struct Spawner {
        outcomes: Vec<(TaskId, TaskOutcome)>,
        spawn_on_action: bool,
    }

    impl KoboApp for Spawner {
        fn on_start(&mut self, context: &mut Context) {
            context.spawn(Task::Sleep { seconds: 1 });
        }

        fn on_action(&mut self, context: &mut Context, _action: ActionId) {
            if self.spawn_on_action {
                context.spawn(Task::Sleep { seconds: 1 });
            }
        }

        fn on_task(&mut self, _context: &mut Context, task: TaskId, outcome: TaskOutcome) {
            self.outcomes.push((task, outcome));
        }
    }

    fn spawned(commands: &[Command]) -> Vec<TaskId> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .collect()
    }

    /// An application that asks for one retryable fetch and records what it is
    /// told, so a test can assert on what reached it and, more importantly,
    /// what did not.
    #[derive(Default)]
    struct Fetcher {
        asked: Option<TaskId>,
        outcomes: Vec<(TaskId, TaskOutcome)>,
        cancel_on_action: bool,
    }

    impl Fetcher {
        fn work() -> Task {
            Task::Fetch {
                url: "https://example.invalid/feed".into(),
                offset: 0,
                max_bytes: 1024,
                credential: None,
                headers: Vec::new(),
            }
        }
    }

    impl KoboApp for Fetcher {
        fn on_start(&mut self, context: &mut Context) {
            self.asked = context.spawn_retrying(Self::work());
        }

        fn on_action(&mut self, context: &mut Context, _action: ActionId) {
            if self.cancel_on_action {
                if let Some(task) = self.asked {
                    context.cancel(task);
                }
            }
        }

        fn on_task(&mut self, _context: &mut Context, task: TaskId, outcome: TaskOutcome) {
            self.outcomes.push((task, outcome));
        }
    }

    fn spawned_work(commands: &[Command]) -> Vec<(TaskId, Task)> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn { task, work } => Some((*task, work.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_retryable_failure_naps_and_tries_again_without_telling_the_application() {
        // The radio powers down when idle. The first request after a while
        // sitting on a page fails while the interface comes up and succeeds
        // moments later, so reporting that first failure would tell the reader
        // they are offline at the moment they are not.
        let mut runner = AppRunner::new(Fetcher::default());
        let first = spawned_work(&runner.start());
        assert_eq!(first.len(), 1);
        let held = first[0].0;

        let nap = spawned_work(&runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline)));
        assert_eq!(
            nap,
            vec![(
                TaskId(2),
                Task::Sleep {
                    seconds: RETRY_DELAY_SECONDS
                }
            )]
        );
        assert!(runner.app().outcomes.is_empty(), "the failure was reported");

        let again =
            spawned_work(&runner.task_outcome(TaskId(2), TaskOutcome::Completed(Vec::new())));
        assert_eq!(again, vec![(TaskId(3), Fetcher::work())]);
        assert!(runner.app().outcomes.is_empty());
    }

    #[test]
    fn a_second_attempt_reports_under_the_identifier_the_application_holds() {
        // Otherwise every application would have to keep a set of identifiers
        // rather than one, and match an answer it never asked for.
        let mut runner = AppRunner::new(Fetcher::default());
        runner.start();
        let held = runner.app().asked.expect("asked");
        runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline));
        runner.task_outcome(TaskId(2), TaskOutcome::Completed(Vec::new()));
        runner.task_outcome(TaskId(3), TaskOutcome::Completed(b"ok".to_vec()));
        let outcomes = &runner.app().outcomes;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0, held);
        assert!(matches!(
            outcomes[0].1,
            TaskOutcome::Completed(ref body) if body == b"ok"
        ));
    }

    #[test]
    fn a_second_failure_is_reported_rather_than_retried_forever() {
        let mut runner = AppRunner::new(Fetcher::default());
        runner.start();
        let held = runner.app().asked.expect("asked");
        runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline));
        runner.task_outcome(TaskId(2), TaskOutcome::Completed(Vec::new()));
        let commands = runner.task_outcome(TaskId(3), TaskOutcome::Failed(TaskError::Offline));
        assert!(spawned_work(&commands).is_empty(), "retried a third time");
        assert_eq!(runner.app().outcomes.len(), 1);
        assert!(matches!(
            runner.app().outcomes[0].1,
            TaskOutcome::Failed(TaskError::Offline)
        ));
    }

    #[test]
    fn a_failure_a_second_attempt_cannot_survive_is_reported_at_once() {
        // A refused permission is not going to change, and a reader watching a
        // spinner is owed an answer rather than a wait for the same no.
        let mut runner = AppRunner::new(Fetcher::default());
        runner.start();
        let held = runner.app().asked.expect("asked");
        let commands = runner.task_outcome(held, TaskOutcome::Failed(TaskError::Denied));
        assert!(spawned_work(&commands).is_empty());
        assert_eq!(runner.app().outcomes.len(), 1);
    }

    #[test]
    fn retrying_holds_one_slot_rather_than_two() {
        let mut runner = AppRunner::new(Fetcher::default());
        runner.start();
        let held = runner.app().asked.expect("asked");
        runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline));
        runner.task_outcome(TaskId(2), TaskOutcome::Completed(Vec::new()));
        assert_eq!(runner.in_flight, 1);
        runner.task_outcome(TaskId(3), TaskOutcome::Completed(Vec::new()));
        assert_eq!(runner.in_flight, 0);
    }

    #[test]
    fn cancelling_reaches_the_attempt_actually_in_the_air() {
        // The application names the task it was given. The runtime is holding
        // a different identifier once a second attempt has started, so an
        // untranslated cancel would name nothing and the fetch would run on.
        let mut runner = AppRunner::new(Fetcher {
            cancel_on_action: true,
            ..Fetcher::default()
        });
        runner.start();
        let held = runner.app().asked.expect("asked");
        runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline));
        runner.task_outcome(TaskId(2), TaskOutcome::Completed(Vec::new()));
        let commands = runner.action(ActionId(1));
        assert!(
            commands.contains(&Command::Cancel(TaskId(3))),
            "cancelled the wrong task: {commands:?}"
        );
    }

    #[test]
    fn cancelling_during_the_nap_stops_it_rather_than_retrying_anyway() {
        let mut runner = AppRunner::new(Fetcher {
            cancel_on_action: true,
            ..Fetcher::default()
        });
        runner.start();
        let held = runner.app().asked.expect("asked");
        runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline));
        let commands = runner.action(ActionId(1));
        assert!(commands.contains(&Command::Cancel(TaskId(2))));
        let after = runner.task_outcome(TaskId(2), TaskOutcome::Cancelled);
        assert!(spawned_work(&after).is_empty(), "retried a cancelled fetch");
        assert_eq!(runner.app().outcomes, vec![(held, TaskOutcome::Cancelled)]);
    }

    #[test]
    fn joining_wifi_opens_settings_without_troubling_the_application() {
        // The application never sees the action and never names the settings
        // application, which is the whole point of the reserved name.
        let mut runner = AppRunner::new(Spawner {
            spawn_on_action: true,
            ..Spawner::default()
        });
        runner.start();
        let commands = runner.action(action_id(JOIN_WIFI));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Launch(name) if name == "settings")),
            "join Wi-Fi did not open settings: {commands:?}"
        );
        assert!(
            spawned(&commands).is_empty(),
            "the application was handed an action it should never see"
        );
    }

    #[test]
    fn an_offline_failure_offers_the_radio_as_well_as_a_retry() {
        let screen = ScreenBuilder::new("t")
            .failure_state(Failure::of(TaskError::Offline), "refresh")
            .build();
        let names: Vec<ActionId> = button_actions(&screen);
        assert!(names.contains(&action_id(JOIN_WIFI)), "{names:?}");
        assert!(names.contains(&action_id("refresh")), "{names:?}");
    }

    #[test]
    fn a_failure_that_will_not_come_right_offers_nothing() {
        // Denied is not retryable, so a Try again would fail identically and a
        // control that cannot help is worse than no control.
        let screen = ScreenBuilder::new("t")
            .failure_state(Failure::of(TaskError::Denied), "refresh")
            .build();
        assert!(button_actions(&screen).is_empty());
    }

    #[test]
    fn a_retryable_failure_that_is_not_the_network_offers_only_a_retry() {
        let screen = ScreenBuilder::new("t")
            .failure_state(Failure::of(TaskError::TimedOut), "refresh")
            .build();
        assert_eq!(button_actions(&screen), vec![action_id("refresh")]);
    }

    /// Every button on a screen, however deeply a band has nested it.
    fn button_actions(screen: &Screen) -> Vec<ActionId> {
        fn walk(nodes: &[Node], found: &mut Vec<ActionId>) {
            for node in nodes {
                match node {
                    Node::Button { action, .. } => found.push(*action),
                    Node::Band { slots, .. } => {
                        for slot in slots {
                            walk(&slot.nodes, found);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&screen.nodes, &mut found);
        found
    }

    #[test]
    fn plain_spawn_is_left_alone() {
        // Only work handed over through spawn_retrying is tried twice.
        let mut runner = AppRunner::new(Spawner::default());
        let held = spawned(&runner.start())[0];
        let commands = runner.task_outcome(held, TaskOutcome::Failed(TaskError::Offline));
        assert!(spawned_work(&commands).is_empty());
        assert_eq!(runner.app().outcomes.len(), 1);
    }

    #[test]
    fn task_identifiers_stay_unique_across_separate_callbacks() {
        // A fresh Context is built for every callback. With the counters living
        // there, the second callback to spawn work handed out an identifier
        // already in use, and the two tasks would have reported back as one.
        let mut runner = AppRunner::new(Spawner {
            spawn_on_action: true,
            ..Spawner::default()
        });
        let first = spawned(&runner.start());
        let second = spawned(&runner.action(ActionId(1)));
        let third = spawned(&runner.action(ActionId(2)));
        assert_eq!(first, vec![TaskId(1)]);
        assert_eq!(second, vec![TaskId(2)]);
        assert_eq!(third, vec![TaskId(3)]);
    }

    #[test]
    fn work_in_flight_is_capped_rather_than_queued_without_limit() {
        struct Greedy(usize);
        impl KoboApp for Greedy {
            fn on_start(&mut self, context: &mut Context) {
                for _ in 0..MAX_TASKS_IN_FLIGHT + 5 {
                    if context.spawn(Task::Sleep { seconds: 1 }).is_some() {
                        self.0 += 1;
                    }
                }
            }
            fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        }
        let mut runner = AppRunner::new(Greedy(0));
        let commands = runner.start();
        assert_eq!(spawned(&commands).len(), MAX_TASKS_IN_FLIGHT);
        assert_eq!(runner.app().0, MAX_TASKS_IN_FLIGHT);
    }

    #[test]
    fn a_settled_task_frees_its_slot() {
        struct Filler;
        impl KoboApp for Filler {
            fn on_start(&mut self, context: &mut Context) {
                while context.spawn(Task::Sleep { seconds: 1 }).is_some() {}
            }
            fn on_action(&mut self, context: &mut Context, _action: ActionId) {
                context.spawn(Task::Sleep { seconds: 1 });
            }
            fn on_task(&mut self, _context: &mut Context, _task: TaskId, _outcome: TaskOutcome) {}
        }
        let mut runner = AppRunner::new(Filler);
        runner.start();
        assert_eq!(runner.tasks_in_flight(), MAX_TASKS_IN_FLIGHT);
        // Nothing has reported back, so there is still no room.
        assert!(spawned(&runner.action(ActionId(1))).is_empty());
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(Vec::new()));
        assert_eq!(spawned(&runner.action(ActionId(2))).len(), 1);
    }

    #[test]
    fn a_cancelled_task_still_reaches_the_application() {
        let mut runner = AppRunner::new(Spawner::default());
        runner.start();
        runner.task_outcome(TaskId(1), TaskOutcome::Cancelled);
        assert_eq!(
            runner.app().outcomes,
            vec![(TaskId(1), TaskOutcome::Cancelled)]
        );
    }

    #[test]
    fn a_failed_task_still_reaches_the_application() {
        let mut runner = AppRunner::new(Spawner::default());
        runner.start();
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Denied));
        assert_eq!(
            runner.app().outcomes,
            vec![(TaskId(1), TaskOutcome::Failed(TaskError::Denied))]
        );
    }

    #[test]
    fn a_callback_that_overruns_the_deadline_is_reported() {
        // The runtime cannot stop a callback from blocking, because it is the
        // callback that holds the thread. What it can do is refuse to let the
        // overrun go unnoticed.
        struct Slow;
        impl KoboApp for Slow {
            fn on_start(&mut self, _context: &mut Context) {
                std::thread::sleep(CALLBACK_DEADLINE + Duration::from_millis(60));
            }
            fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        }
        let commands = AppRunner::new(Slow).start();
        assert!(matches!(
            commands.first(),
            Some(Command::Log {
                level: LogLevel::Warn,
                message,
            }) if message.contains("deadline")
        ));
    }

    #[test]
    fn a_prompt_callback_is_not_reported() {
        let commands = AppRunner::new(Spawner::default()).start();
        assert!(!commands.iter().any(|command| matches!(
            command,
            Command::Log {
                level: LogLevel::Warn,
                ..
            }
        )));
    }
}

/// Connects to the runtime and runs an application until it exits.
///
/// This is the whole of an application's `main`. It exists because every
/// application would otherwise hand-roll the same event loop, and each
/// hand-rolled copy is a chance to forget one of the things that has to be
/// right: collecting outstanding device answers before leaving, forwarding
/// every command, honouring a runtime request to exit, and never blocking.
///
/// The socket path comes from the environment the runtime provides, so an
/// application never names a path and never has to be told where it is running.
///
/// # Errors
///
/// Returns the first transport error. There is deliberately no retry: if the
/// runtime is gone, the screen belongs to something else now.
pub fn run<A: KoboApp>(name: &str, app: A) -> Result<(), ClientError> {
    let socket = std::env::var("KOBO_SOCKET").map_err(|_| ClientError::MissingSocket)?;
    run_on(name, app, Path::new(&socket))
}

/// Runs an application against a specific runtime socket.
///
/// # Errors
///
/// Returns the first transport error.
pub fn run_on<A: KoboApp>(name: &str, app: A, socket: &Path) -> Result<(), ClientError> {
    let mut client = Client::connect(socket, name)?;
    // The same typeface the runtime lays out with, so an application that
    // measures its own text agrees with what will actually be drawn. Failure
    // is not fatal: both sides then fall back to the built-in bitmap, which is
    // still one shared answer rather than two different ones.
    #[cfg(feature = "text")]
    let _ = kobo_text::install(client.metrics());
    let mut runner = AppRunner::with_metrics(app, client.metrics());
    client.send_commands(runner.start())?;

    // A test harness needs the application to settle and leave rather than wait
    // for a touch that will never come.
    let oneshot = std::env::var_os("KOBO_SIM_ONESHOT").is_some();
    if oneshot {
        while runner.outstanding_answers() > 0 {
            match client.next_event()? {
                ClientEvent::Device(result) => {
                    client.send_commands(runner.device_result(result))?;
                }
                ClientEvent::Task { task, outcome } => {
                    client.send_commands(runner.task_outcome(task, outcome))?;
                }
                ClientEvent::Store(result) => {
                    client.send_commands(runner.store_result(result))?;
                }
                ClientEvent::Lifecycle(state) => {
                    client.send_commands(runner.lifecycle(state))?;
                }
                ClientEvent::Shell(event) => {
                    client.send_commands(runner.shell_event(event))?;
                }
                ClientEvent::CoverChanged(present) => {
                    client.send_commands(runner.cover_changed(present))?;
                }
                ClientEvent::PageTurn(forward) => {
                    client.send_commands(runner.page_turn(forward))?;
                }
                ClientEvent::Action(_) | ClientEvent::TextHold { .. } | ClientEvent::Exit => break,
            }
        }
        client.send_commands([Command::Exit])?;
        return Ok(());
    }

    loop {
        let commands = match client.next_event()? {
            ClientEvent::Action(action) => runner.action(action),
            ClientEvent::TextHold { action, hit } => runner.text_hold(action, hit),
            ClientEvent::Device(result) => runner.device_result(result),
            ClientEvent::Task { task, outcome } => runner.task_outcome(task, outcome),
            ClientEvent::Store(result) => runner.store_result(result),
            ClientEvent::Lifecycle(state) => runner.lifecycle(state),
            ClientEvent::Shell(event) => runner.shell_event(event),
            ClientEvent::CoverChanged(present) => runner.cover_changed(present),
            ClientEvent::PageTurn(forward) => runner.page_turn(forward),
            ClientEvent::Exit => {
                // The runtime is taking the screen back. Give the application
                // its exit callback, then go, rather than arguing about it.
                let _ = client.send_commands(runner.exit());
                return Ok(());
            }
        };
        let leaving = commands
            .iter()
            .any(|command| matches!(command, Command::Exit));
        client.send_commands(commands)?;
        if leaving {
            return Ok(());
        }
    }
}
