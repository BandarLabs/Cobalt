//! A Muse gadget's face, in e-ink.
//!
//! Muse, the assistant, decides what this reader shows: a resting line, a page
//! to read, a question with answers to tap. A bridge on a computer keeps the
//! link to Muse and holds the current screen; this application long-polls it
//! over HTTPS, draws a screen only when its revision moves, and posts the tap
//! back. Nothing animates and nothing repaints for a quiet poll, which is what
//! an e-ink panel is good at.
//!
//! Pairing is typed once and remembered: the bridge's address, then the
//! six-character code it printed. The code is exchanged for a token that is
//! stored with the address. The connection is TLS against a root the owner
//! installed with `kobo trust set muse-panel`.

mod wire;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, Header, KoboApp,
    ParagraphPresentation, PictureHandle, RichTextSpan, Screen, ScreenBuilder, Space, StoreResult,
    Task, TaskId, TaskOutcome, TextPresentation, TilePicture,
};
use std::process::ExitCode;
use wire::{plain, Block, Content, Live, Question, Reply, Span};

const PICTURE: PictureHandle = PictureHandle(1);
/// The most a picture may weigh on the wire; kobo-image refuses more anyway.
const MAX_PICTURE: u32 = 4 * 1024 * 1024;
/// What one picture may hold on the way to the panel (kobo-protocol's budget).
const MAX_COLOUR_BYTES: u64 = 4 * 1072 * 1448;
const TITLE: &str = "Muse";
const PAIRED: &str = "paired";
const REPAIR: &str = "repair";
const PREVIOUS: &str = "previous-page";
const NEXT: &str = "next-page";
const CHOOSE: &str = "choose-";

/// Typed when the owner gives a bare address.
const DEFAULT_PORT: &str = "8473";
const CODE_LENGTH: usize = 6;

/// How long the bridge holds a poll when nothing has changed.
const POLL_WAIT: &str = "25";
const MAX_REPLY: u32 = 128 * 1024;
const MAX_SMALL_REPLY: u32 = 4 * 1024;
/// After a failed poll, so a dead network is not spun on.
const NAP_SECONDS: u32 = 10;
/// Between polls that came back at once, so a bridge that does not hold the
/// request cannot turn this into a busy loop on the radio.
const BETWEEN_POLLS: u32 = 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Opening,
    Address,
    Code,
    /// The code is on its way to the bridge.
    Pairing,
    /// Paired: the screen Muse chose.
    Live,
    /// A tap is on its way to the bridge.
    Sending,
}

#[derive(Default)]
struct Panel {
    view: View,
    keyboard: Keyboard,
    address: String,
    token: String,
    trouble: Option<String>,
    live: Option<Live>,
    /// Reader-local hour and minute at which the bridge last returned a reply
    /// this app validated and drew. Never set from a failed or unreadable poll.
    updated: Option<(u8, u8)>,
    /// The reader's UTC offset in minutes, from `KOBO_UTC_OFFSET_MINUTES`.
    /// None when the variable is missing or not a number. The time is then
    /// computed at UTC, as the other apps do, and labelled "UTC" on screen so
    /// it is never passed off as local time.
    offset_minutes: Option<i16>,
    /// The revision this reader last asked about.
    rev: Option<u64>,
    /// A page or question the reader put away with Back. It stays away until
    /// the revision moves.
    dismissed: Option<u64>,
    page: usize,
    poll: Option<TaskId>,
    nap: Option<TaskId>,
    send: Option<TaskId>,
    pair: Option<TaskId>,
    hello: Option<TaskId>,
    /// The picture Muse drew, on its way, and then on the panel.
    fetching: Option<TaskId>,
    picture: Option<TilePicture>,
    unreadable: bool,
    identity: Option<kobo_sdk::DeviceIdentity>,
    battery: Option<u8>,
    said_hello: bool,
}

impl Panel {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context).with_own_back(self.owns_back()));
    }

    /// Back retreats a step inside the flow rather than leaving.
    fn owns_back(&self) -> bool {
        match self.view {
            View::Code => true,
            View::Live => self.visible_content().is_some(),
            _ => false,
        }
    }

    /// The page, question or picture on show, unless the reader put it away.
    fn visible_content(&self) -> Option<&Content> {
        let live = self.live.as_ref()?;
        if self.dismissed == Some(live.rev) || live.content == Content::Status {
            return None;
        }
        Some(&live.content)
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("muse-opening")
                .top_bar(TITLE)
                .activity("Opening Muse", None)
                .build(),
            View::Address => self.address_screen(),
            View::Code => self.code_screen(),
            View::Pairing => ScreenBuilder::new("muse-pairing")
                .top_bar(TITLE)
                .activity("Pairing with Muse", None)
                .build(),
            View::Sending => ScreenBuilder::new("muse-sending")
                .top_bar(TITLE)
                .activity("Sending your answer", None)
                .build(),
            View::Live => match self.visible_content() {
                Some(Content::Page { title, blocks }) => self.page(context, title, blocks),
                Some(Content::Ask(question)) => self.asking(context, question),
                Some(Content::Image(_)) => self.picture_screen(),
                _ => self.resting(),
            },
        }
    }

    fn picture_screen(&self) -> Screen {
        let screen = ScreenBuilder::new("muse-picture");
        match self.picture {
            Some(picture) => screen
                .full_bleed_picture(picture, 500)
                .top_bar("From Muse")
                .build()
                .with_own_back(true)
                .with_reading(true)
                .with_auto_hidden_top_bar(true),
            None if self.unreadable => screen
                .top_bar(TITLE)
                .banner(
                    BannerLevel::Attention,
                    "Muse sent a picture this reader could not open.",
                )
                .build(),
            None => screen
                .top_bar(TITLE)
                .activity("Opening the picture", None)
                .build(),
        }
    }

    /// Asks the bridge for the bytes of the picture Muse chose.
    fn fetch_picture(&mut self, context: &mut Context) {
        self.picture = None;
        self.unreadable = false;
        context.drop_picture(PICTURE);
        let Some(Content::Image(picture)) = self.live.as_ref().map(|live| &live.content) else {
            return;
        };
        let url = self.url(&format!("blob/{}", picture.blob));
        self.fetching = context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_PICTURE,
            credential: None,
            headers: self.auth(),
        });
    }

    fn on_picture(&mut self, context: &mut Context, outcome: &TaskOutcome) {
        let TaskOutcome::Completed(bytes) = outcome else {
            self.unreadable = true;
            self.show(context);
            return;
        };
        let colour = self
            .identity
            .as_ref()
            .is_some_and(kobo_sdk::DeviceIdentity::colour_panel);
        let metrics = context.metrics();
        let decoded = if colour {
            kobo_image::decode_colour(bytes)
        } else {
            kobo_image::decode(bytes)
        };
        // Colour costs three bytes a pixel against a fixed budget, so a
        // colour panel's picture is shrunk until it fits.
        let (mut width, mut height) = (
            u32::try_from(metrics.width).unwrap_or(1),
            u32::try_from(metrics.height).unwrap_or(1),
        );
        while colour && u64::from(width) * u64::from(height) * 3 > MAX_COLOUR_BYTES {
            width = width * 99 / 100;
            height = height * 99 / 100;
        }
        self.picture = decoded
            .and_then(|picture| picture.fit_enlarging(width, height))
            .ok()
            .and_then(|picture| {
                let (w, h) = (picture.width(), picture.height());
                // A picture that does not match the panel's shape sits in the
                // middle of white paper instead of hugging the top edge.
                if colour {
                    let rgb = picture.into_colour()?;
                    context.put_colour_picture(
                        PICTURE,
                        width,
                        height,
                        centred(&rgb, 3, (w, h), (width, height)),
                    )
                } else {
                    let grey = picture.into_grey();
                    context.put_picture(
                        PICTURE,
                        width,
                        height,
                        centred(&grey, 1, (w, h), (width, height)),
                    )
                }
            });
        self.unreadable = self.picture.is_none();
        self.show(context);
    }

    fn address_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("muse-address")
            .top_bar("Set up Muse")
            .heading("Your computer's address")
            .text("Run kobo-bridge init on the computer that talks to Muse, then type the address it shows.");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .field("address.box", self.keyboard.text(), "192.168.1.20:8473")
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Next")
            .build()
    }

    fn code_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("muse-code")
            .top_bar("Set up Muse")
            .heading("Pairing code");
        screen = if let Some(trouble) = &self.trouble {
            screen.banner(BannerLevel::Attention, trouble.clone())
        } else {
            screen.text("Type the six characters shown under the address.")
        };
        let typed: Vec<char> = self.keyboard.text().trim().chars().collect();
        let boxes = (0..CODE_LENGTH).map(|slot| {
            (
                format!("code.{slot}"),
                typed
                    .get(slot)
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default(),
            )
        });
        screen
            .grid(6, true, boxes)
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Pair")
            .build()
    }

    /// The face Muse leaves when it has nothing to show.
    fn resting(&self) -> Screen {
        let (line, detail) = self
            .live
            .as_ref()
            .map_or(("", ""), |live| (live.line.as_str(), live.detail.as_str()));
        let zone = if self.offset_minutes.is_some() {
            ""
        } else {
            " UTC"
        };
        let stamp = self
            .updated
            .map(|(hour, minute)| format!("Updated {hour:02}:{minute:02}{zone}"));
        let detail = match (detail.is_empty(), stamp) {
            (_, None) => detail.to_owned(),
            (true, Some(stamp)) => stamp,
            (false, Some(stamp)) => format!("{detail}\n{stamp}"),
        };
        let mut screen = ScreenBuilder::new("muse-resting")
            .top_bar(TITLE)
            .top_bar_glyph(REPAIR, "Pairing", Glyph::Settings)
            .splash(
                Some(Glyph::Chat),
                if !line.is_empty() {
                    line
                } else if self.live.is_some() {
                    "Nothing new from Muse"
                } else {
                    "Waiting for Muse"
                },
                detail,
            );
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen.build()
    }

    fn ask_screen(
        &self,
        question: &Question,
        shown: &[usize],
        page: usize,
        pages: usize,
    ) -> Screen {
        let mut screen = ScreenBuilder::new("muse-asking")
            .top_bar("Muse asks")
            .heading(question.text.clone());
        if !question.context.is_empty() {
            screen = screen.text(question.context.clone());
        }
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen = screen.spacer(Space::Small).rows(shown.iter().map(|&index| {
            (
                format!("{CHOOSE}{index}"),
                question.choices[index].label.clone(),
                String::new(),
                Glyph::Circle,
            )
        }));
        if pages > 1 {
            screen = screen.page_turns(PREVIOUS, NEXT).page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages).unwrap_or(u16::MAX),
            );
        }
        screen.build()
    }

    /// Choices a screen can hold, in order. At the largest text size a long
    /// list turns pages instead of pushing an answer off the panel.
    fn ask_pages(&self, context: &Context, question: &Question) -> Vec<Vec<usize>> {
        let fits = |shown: &[usize]| {
            !self
                .ask_screen(question, shown, 0, 2)
                .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        };
        let mut pages: Vec<Vec<usize>> = Vec::new();
        for index in 0..question.choices.len() {
            if let Some(last) = pages.last_mut() {
                last.push(index);
                if fits(last) {
                    continue;
                }
                last.pop();
            }
            pages.push(vec![index]);
        }
        pages
    }

    fn asking(&self, context: &Context, question: &Question) -> Screen {
        let pages = self.ask_pages(context, question);
        let page = self.page.min(pages.len().saturating_sub(1));
        self.ask_screen(question, &pages[page], page, pages.len())
    }

    // -- Pages ---------------------------------------------------------------

    fn page_screen(&self, title: &str, blocks: &[Block], page: usize, pages: usize) -> Screen {
        let mut screen = ScreenBuilder::new("muse-page")
            .top_bar(TITLE)
            .heading(if title.is_empty() { "Note" } else { title });
        for block in blocks {
            screen = add_block(screen, block, false);
        }
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen = screen.fill();
        if pages > 1 {
            screen = screen.page_turns(PREVIOUS, NEXT).page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages).unwrap_or(u16::MAX),
            );
        }
        screen.build()
    }

    fn page(&self, context: &Context, title: &str, blocks: &[Block]) -> Screen {
        let pages = self.paginate(context, title, blocks);
        let page = self.page.min(pages.len().saturating_sub(1));
        self.page_screen(title, &pages[page], page, pages.len())
    }

    fn page_count(&self, context: &Context) -> usize {
        match self.visible_content() {
            Some(Content::Page { title, blocks }) => self.paginate(context, title, blocks).len(),
            Some(Content::Ask(question)) => self.ask_pages(context, question).len(),
            _ => 1,
        }
    }

    /// Fills each screen with as many blocks as the renderer will accept,
    /// measured against the real screen and never a guessed line count.
    fn paginate(&self, context: &Context, title: &str, blocks: &[Block]) -> Vec<Vec<Block>> {
        let fits = |chunk: &[Block]| {
            !self
                .page_screen(title, chunk, 0, 2)
                .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        };
        let mut pages: Vec<Vec<Block>> = Vec::new();
        for block in explode(blocks) {
            if let Some(last) = pages.last_mut() {
                last.push(block.clone());
                if fits(last) {
                    continue;
                }
                last.pop();
            }
            if fits(std::slice::from_ref(&block)) {
                pages.push(vec![block]);
                continue;
            }
            for part in split_text_block(&block, |piece| fits(std::slice::from_ref(piece))) {
                pages.push(vec![part]);
            }
        }
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        pages
    }

    // -- Network -------------------------------------------------------------

    /// The paired token travels in a header, never in the URL, so it does not
    /// end up in URL-based logs or history. Headers can still be logged by
    /// anything that terminates the connection.
    fn auth(&self) -> Vec<Header> {
        vec![Header::new("X-Muse-Panel-Token", self.token.clone())]
    }

    fn url(&self, path: &str) -> String {
        format!("https://{}/v1/{path}", self.address)
    }

    fn start_poll(&mut self, context: &mut Context) {
        if self.poll.is_some() {
            return;
        }
        let rev = self.rev.map_or(String::new(), |rev| format!("rev={rev}&"));
        let url = format!("{}?{rev}wait={POLL_WAIT}", self.url("screen"));
        self.poll = context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY,
            credential: None,
            headers: self.auth(),
        });
    }

    fn post(&self, context: &mut Context, path: &str, body: String) -> Option<TaskId> {
        context.spawn(Task::Post {
            url: self.url(path),
            body,
            content_type: "application/json".to_owned(),
            credential: None,
            headers: self.auth(),
            max_bytes: MAX_SMALL_REPLY,
        })
    }

    fn say_hello(&mut self, context: &mut Context) {
        if self.hello.is_some() || self.token.is_empty() {
            return;
        }
        let metrics = context.metrics();
        let mut body = kobo_json::ObjectBuilder::new()
            .set("w", metrics.width)
            .set("h", metrics.height);
        if let Some(identity) = &self.identity {
            body = body.set("colour", identity.colour_panel());
        }
        if let Some(percent) = self.battery {
            body = body.set("battery", u32::from(percent));
        }
        self.hello = self.post(context, "hello", body.build().to_json());
        self.said_hello = true;
    }

    fn on_poll(&mut self, context: &mut Context, outcome: TaskOutcome) {
        if self.view != View::Live {
            return;
        }
        match outcome {
            TaskOutcome::Completed(bytes) => {
                let repaint = self.trouble.take().is_some();
                match wire::read(&bytes) {
                    Some(Reply::Changed(live)) if Some(live.rev) != self.rev => {
                        self.rev = Some(live.rev);
                        self.dismissed = None;
                        self.page = 0;
                        self.updated = local_time(self.offset_minutes.unwrap_or(0));
                        self.live = Some(live);
                        if matches!(
                            self.live.as_ref().map(|live| &live.content),
                            Some(Content::Image(_))
                        ) {
                            self.fetch_picture(context);
                        }
                        self.show(context);
                    }
                    _ if repaint => self.show(context),
                    _ => {}
                }
                self.nap = context.spawn(Task::Sleep {
                    seconds: BETWEEN_POLLS,
                });
            }
            TaskOutcome::Failed(error) => {
                self.trouble = Some(match error {
                    kobo_sdk::TaskError::NotFound | kobo_sdk::TaskError::Unauthorized => {
                        "Muse no longer knows this reader. Open Pairing at the top to connect again.".to_owned()
                    }
                    other => Failure::of(other).advice.to_owned(),
                });
                self.show(context);
                self.nap = context.spawn(Task::Sleep {
                    seconds: NAP_SECONDS,
                });
            }
            TaskOutcome::Cancelled => {}
        }
    }

    fn on_pair(&mut self, context: &mut Context, outcome: &TaskOutcome) {
        let token = match outcome {
            TaskOutcome::Completed(bytes) => std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| kobo_json::parse(text).ok())
                .and_then(|body| {
                    body.get("token")
                        .and_then(kobo_json::Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|token| !token.is_empty()),
            _ => None,
        };
        if let Some(token) = token {
            self.token = token;
            let record = format!("{}\n{}", self.address, self.token);
            context.store().save(PAIRED, record.into_bytes());
            self.view = View::Live;
            self.trouble = None;
            self.live = None;
            self.rev = None;
            self.poll = None;
            self.show(context);
            self.start_poll(context);
            self.say_hello(context);
            return;
        }
        self.view = View::Code;
        self.trouble = Some(match outcome {
            TaskOutcome::Failed(
                kobo_sdk::TaskError::NotFound | kobo_sdk::TaskError::Unauthorized,
            ) => "That code did not work. Check it against your computer, or wait a minute if you have tried a few times."
                .to_owned(),
            TaskOutcome::Failed(error) => Failure::of(*error).advice.to_owned(),
            _ => "Muse sent something this reader did not understand.".to_owned(),
        });
        self.show(context);
    }

    fn on_sent(&mut self, context: &mut Context, outcome: &TaskOutcome) {
        self.view = View::Live;
        match outcome {
            TaskOutcome::Completed(_) => {
                self.trouble = None;
                // The bridge moves the screen on once it has the answer; the
                // next poll draws it. Until then the question stays put away.
                self.dismissed = self.live.as_ref().map(|live| live.rev);
            }
            TaskOutcome::Failed(error) => {
                // A tap that timed out may still have landed. The next poll
                // says which, so nothing is resent from here.
                self.trouble = Some(match error {
                    kobo_sdk::TaskError::NotFound | kobo_sdk::TaskError::Unauthorized => {
                        "Muse has already moved on from that question.".to_owned()
                    }
                    other => Failure::of(*other).advice.to_owned(),
                });
            }
            TaskOutcome::Cancelled => {}
        }
        self.show(context);
        self.start_poll(context);
    }

    fn choose(&mut self, context: &mut Context, index: usize) {
        let Some(Live {
            content: Content::Ask(question),
            ..
        }) = &self.live
        else {
            return;
        };
        let Some(choice) = question.choices.get(index) else {
            return;
        };
        let body = kobo_json::ObjectBuilder::new()
            .set("ask_id", question.id.as_str())
            .set("choice", choice.id.as_str())
            .build()
            .to_json();
        if self.send.is_some() {
            return;
        }
        if let Some(task) = self.post(context, "event", body) {
            self.send = Some(task);
            self.view = View::Sending;
            self.trouble = None;
            self.show(context);
        }
    }

    // -- Pairing -------------------------------------------------------------

    fn accept_address(&mut self, typed: &str) -> Option<String> {
        let typed: String = typed.split_whitespace().collect();
        if typed.is_empty() {
            return None;
        }
        let address = if typed.contains(':') {
            typed
        } else {
            format!("{typed}:{DEFAULT_PORT}")
        };
        let plausible = address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
        if plausible {
            self.trouble = None;
            Some(address)
        } else {
            self.trouble = Some("Type the address your computer shows.".to_owned());
            None
        }
    }

    fn typing(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(pressed) = self.keyboard.press(action) else {
            return false;
        };
        match pressed {
            Pressed::Edited | Pressed::Shifted => {
                if self.view == View::Code && self.keyboard.text().chars().count() > CODE_LENGTH {
                    let kept: String = self.keyboard.text().chars().take(CODE_LENGTH).collect();
                    self.keyboard = Keyboard::with_text(kept);
                }
                if self.view == View::Code
                    && self.keyboard.text().trim().chars().count() == CODE_LENGTH
                {
                    self.trouble = None;
                }
                self.show(context);
            }
            Pressed::Submitted => match self.view {
                View::Address => {
                    let typed = self.keyboard.text().to_owned();
                    if let Some(address) = self.accept_address(&typed) {
                        self.address = address;
                        self.keyboard.clear();
                        self.view = View::Code;
                    }
                    self.show(context);
                }
                View::Code => {
                    let code = self.keyboard.text().trim().to_uppercase();
                    if code.chars().count() != CODE_LENGTH {
                        self.trouble = Some("The code has six characters.".to_owned());
                        self.show(context);
                        return true;
                    }
                    self.keyboard.clear();
                    let body = kobo_json::ObjectBuilder::new()
                        .set("code", code.as_str())
                        .build()
                        .to_json();
                    self.pair = context.spawn(Task::Post {
                        url: self.url("pair"),
                        body,
                        content_type: "application/json".to_owned(),
                        credential: None,
                        headers: Vec::new(),
                        max_bytes: MAX_SMALL_REPLY,
                    });
                    self.poll = None;
                    self.view = View::Pairing;
                    self.trouble = None;
                    self.show(context);
                }
                _ => {}
            },
        }
        true
    }
}

// -- Drawing blocks ----------------------------------------------------------

/// `pixels` (`inner` wide and high, `depth` bytes a pixel) laid in the middle
/// of a white canvas of `outer`.
fn centred(pixels: &[u8], depth: usize, inner: (u32, u32), outer: (u32, u32)) -> Vec<u8> {
    let (iw, ih) = (inner.0 as usize, inner.1 as usize);
    let (ow, oh) = (outer.0 as usize, outer.1 as usize);
    let mut canvas = vec![255; ow * oh * depth];
    let (left, top) = (ow.saturating_sub(iw) / 2, oh.saturating_sub(ih) / 2);
    for row in 0..ih.min(oh) {
        let from = row * iw * depth;
        let to = ((top + row) * ow + left) * depth;
        let length = iw.min(ow) * depth;
        canvas[to..to + length].copy_from_slice(&pixels[from..from + length]);
    }
    canvas
}

fn rich(spans: &[Span], prefix: &str) -> (String, Vec<RichTextSpan>) {
    let mut text = prefix.to_owned();
    let mut runs = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        if span.strong && !span.text.is_empty() {
            runs.push(RichTextSpan {
                start,
                end: text.len(),
                presentation: TextPresentation {
                    strong: true,
                    ..TextPresentation::default()
                },
            });
        }
    }
    (text, runs)
}

fn add_prose(screen: ScreenBuilder, spans: &[Span], prefix: &str) -> ScreenBuilder {
    let (text, runs) = rich(spans, prefix);
    if runs.is_empty() {
        screen.text(text)
    } else {
        screen.rich_text(text, runs, ParagraphPresentation::default())
    }
}

fn add_block(screen: ScreenBuilder, block: &Block, titled: bool) -> ScreenBuilder {
    match block {
        Block::Heading { level, spans } => {
            let depth = if titled { 2 } else { *level };
            screen.heading_at_level(depth, plain(spans))
        }
        Block::Paragraph(spans) => add_prose(screen, spans, ""),
        Block::List(items) => items
            .iter()
            .fold(screen, |screen, item| add_prose(screen, item, "\u{2022} ")),
        Block::Quote(spans) => screen.quote(0, plain(spans)),
        Block::Rule => screen.divider(),
        Block::Image { alt, .. } => screen.secondary(if alt.is_empty() {
            "Image".to_owned()
        } else {
            alt.clone()
        }),
    }
}

/// One list item per block, so a long list can break between items.
fn explode(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .flat_map(|block| match block {
            Block::List(items) if items.len() > 1 => items
                .iter()
                .map(|item| Block::List(vec![item.clone()]))
                .collect(),
            other => vec![other.clone()],
        })
        .collect()
}

/// The part of `spans` between two byte offsets of its plain text.
fn slice(spans: &[Span], from: usize, to: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = 0;
    for span in spans {
        let end = at + span.text.len();
        let (lo, hi) = (from.max(at), to.min(end));
        if lo < hi {
            out.push(Span {
                text: span.text[lo - at..hi - at].to_owned(),
                strong: span.strong,
            });
        }
        at = end;
    }
    out
}

fn spans_of(block: &Block) -> Option<&[Span]> {
    match block {
        Block::Paragraph(spans) | Block::Quote(spans) | Block::Heading { spans, .. } => Some(spans),
        Block::List(items) if items.len() == 1 => Some(&items[0]),
        _ => None,
    }
}

fn with_spans(block: &Block, spans: Vec<Span>) -> Block {
    match block {
        Block::Quote(_) => Block::Quote(spans),
        Block::Heading { level, .. } => Block::Heading {
            level: *level,
            spans,
        },
        Block::List(_) => Block::List(vec![spans]),
        _ => Block::Paragraph(spans),
    }
}

/// A block too tall for any screen, cut into pieces that each fit. Nothing is
/// dropped; a cut lands on a word boundary where there is one.
fn split_text_block(block: &Block, mut fits: impl FnMut(&Block) -> bool) -> Vec<Block> {
    let Some(spans) = spans_of(block) else {
        return vec![block.clone()];
    };
    let text = plain(spans);
    let mut parts = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let rest = &text[start..];
        let end = fitting_prefix(rest, |candidate| {
            fits(&with_spans(
                block,
                slice(spans, start, start + candidate.len()),
            ))
        });
        parts.push(with_spans(block, slice(spans, start, start + end)));
        start += end;
    }
    parts
}

/// Keeps UTF-8 and whitespace intact while preferring a word boundary.
fn fitting_prefix(text: &str, mut fits: impl FnMut(&str) -> bool) -> usize {
    let ends = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let (mut low, mut high) = (0, ends.len() - 1);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(&text[..ends[mid]]) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut end = ends[low.max(1)];
    if end < text.len() {
        if let Some((boundary, c)) = text[..end]
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
        {
            if boundary > 0 {
                end = boundary + c.len_utf8();
            }
        }
    }
    end
}

impl KoboApp for Panel {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(PAIRED);
        context.device().read_identity();
        context.device().read_battery();
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let StoreResult::Loaded { key, value } = result else {
            return;
        };
        if key != PAIRED || self.view != View::Opening {
            return;
        }
        let remembered = value
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| {
                let (address, token) = text.split_once('\n')?;
                Some((address.trim().to_owned(), token.trim().to_owned()))
            })
            .filter(|(address, token)| !address.is_empty() && !token.is_empty());
        if let Some((address, token)) = remembered {
            self.address = address;
            self.token = token;
            self.view = View::Live;
            self.show(context);
            self.start_poll(context);
            self.say_hello(context);
        } else {
            self.view = View::Address;
            self.show(context);
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        if let (kobo_sdk::DeviceRequest::ReadIdentity, kobo_sdk::DeviceResult::Identity(identity)) =
            (&request, &result)
        {
            self.identity = Some(identity.clone());
            return;
        }
        if let (
            kobo_sdk::DeviceRequest::ReadBattery,
            kobo_sdk::DeviceResult::Battery { percent, .. },
        ) = (request, result)
        {
            self.battery = Some(percent);
            if self.view == View::Live && !self.said_hello {
                self.say_hello(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            match self.view {
                View::Code => {
                    self.keyboard = Keyboard::with_text(&self.address);
                    self.view = View::Address;
                    self.trouble = None;
                    self.show(context);
                }
                View::Live if self.visible_content().is_some() => {
                    self.dismissed = self.live.as_ref().map(|live| live.rev);
                    self.show(context);
                }
                _ => {}
            }
            return;
        }
        if matches!(self.view, View::Address | View::Code) && self.typing(context, action) {
            return;
        }
        if self.view != View::Live {
            return;
        }
        if action == action_id(REPAIR) {
            self.keyboard = Keyboard::with_text(&self.address);
            self.view = View::Address;
            self.trouble = None;
            self.show(context);
        } else if action == action_id(PREVIOUS) || action == action_id(NEXT) {
            let pages = self.page_count(context);
            let current = self.page.min(pages.saturating_sub(1));
            self.page = if action == action_id(PREVIOUS) {
                current.saturating_sub(1)
            } else {
                current.saturating_add(1).min(pages.saturating_sub(1))
            };
            self.show(context);
        } else if let Some(Content::Ask(question)) = self.visible_content() {
            let found = (0..question.choices.len())
                .find(|index| action == action_id(&format!("{CHOOSE}{index}")));
            if let Some(index) = found {
                self.choose(context, index);
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.poll == Some(task) {
            self.poll = None;
            self.on_poll(context, outcome);
        } else if self.pair == Some(task) {
            self.pair = None;
            self.on_pair(context, &outcome);
        } else if self.send == Some(task) {
            self.send = None;
            self.on_sent(context, &outcome);
        } else if self.fetching == Some(task) {
            self.fetching = None;
            self.on_picture(context, &outcome);
        } else if self.hello == Some(task) {
            self.hello = None;
        } else if self.nap == Some(task) {
            self.nap = None;
            if self.view == View::Live {
                self.start_poll(context);
            }
        }
    }

    fn on_foreground(&mut self, context: &mut Context) {
        if self.view == View::Live && self.poll.is_none() && self.nap.is_none() {
            self.start_poll(context);
        }
    }
}

/// The offset the runtime was started with, if it gave a usable one.
fn reader_offset() -> Option<i16> {
    parse_offset(&std::env::var("KOBO_UTC_OFFSET_MINUTES").ok()?)
}

/// A whole number of minutes the clock accepts (-840 to 840), else None so the
/// labelled UTC fallback applies instead of no time at all.
fn parse_offset(value: &str) -> Option<i16> {
    value
        .trim()
        .parse::<i16>()
        .ok()
        .filter(|minutes| (-840..=840).contains(minutes))
}

fn local_time(offset_minutes: i16) -> Option<(u8, u8)> {
    use kobo_sdk::clock::{Clock, SystemClock};
    SystemClock::new(offset_minutes)
        .ok()?
        .now()
        .ok()?
        .hour_minute()
}

fn main() -> ExitCode {
    match kobo_sdk::run(
        "muse-panel",
        Panel {
            offset_minutes: reader_offset(),
            ..Panel::default()
        },
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("muse-panel: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, StoreRequest, TaskError};
    use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, TextScale, CLARA_BW_METRICS};

    const SCREEN_PAGE: &[u8] = br#"{"rev":7,"kind":"page","status":{"line":"Planning","detail":""},
        "page":{"id":"p","title":"Saturday","blocks":[
          {"t":"h","level":1,"spans":[{"s":"Plan"}]},
          {"t":"p","spans":[{"s":"Gym at nine, then "},{"s":"lunch at one","b":true},{"s":"."}]},
          {"t":"ul","items":[[{"s":"Pick up the parcel"}],[{"s":"Call the plumber"}]]},
          {"t":"q","spans":[{"s":"Rest is a task too."}]},
          {"t":"hr"}]}}"#;
    const SCREEN_ASK: &[u8] = br#"{"rev":8,"kind":"ask","status":{"line":"","detail":""},
        "ask":{"ask_id":"lunch","question":"Move lunch to two?","context":"Gym runs late",
        "choices":[{"id":"c1","label":"Yes"},{"id":"c2","label":"No"},{"id":"c3","label":"Ask me later"}]}}"#;
    const SCREEN_STATUS: &[u8] = br#"{"rev":9,"kind":"status",
        "status":{"line":"Writing your brief","detail":"Back in a few minutes"},
        "page":null,"ask":null,"image":null}"#;

    fn paired() -> (Panel, TaskId) {
        let mut app = Panel::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:8473\ntok123".to_vec()),
            },
        );
        let poll = fetched(&context.take_commands()).expect("a poll starts").0;
        (app, poll)
    }

    fn fetched(commands: &[Command]) -> Option<(TaskId, String)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Fetch { url, .. },
            } => Some((*task, url.clone())),
            _ => None,
        })
    }

    fn posted(commands: &[Command]) -> Option<(TaskId, String, String)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Post { url, body, .. },
            } => Some((*task, url.clone(), body.clone())),
            _ => None,
        })
    }

    fn slept(commands: &[Command]) -> Option<TaskId> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Sleep { .. },
            } => Some(*task),
            _ => None,
        })
    }

    fn painted(commands: &[Command]) -> Option<Screen> {
        commands.iter().rev().find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen.clone()),
            _ => None,
        })
    }

    fn shown(screen: &Screen) -> Vec<String> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    fn deliver(app: &mut Panel, task: TaskId, bytes: &[u8]) -> Vec<Command> {
        let mut context = Context::default();
        app.on_task(&mut context, task, TaskOutcome::Completed(bytes.to_vec()));
        context.take_commands()
    }

    fn act(app: &mut Panel, action: ActionId) -> Vec<Command> {
        let mut context = Context::default();
        app.on_action(&mut context, action);
        context.take_commands()
    }

    /// A long page, to force several screens.
    fn long_page() -> Vec<u8> {
        let paragraph = "A paragraph that says something plain about the afternoon and runs on \
                         long enough that several of them cannot share one panel.";
        let blocks: Vec<String> = (0..14)
            .map(|n| format!(r#"{{"t":"p","spans":[{{"s":"{n}. {paragraph}"}}]}}"#))
            .collect();
        format!(
            r#"{{"rev":12,"kind":"page","status":{{"line":"","detail":""}},
            "page":{{"id":"long","title":"Notes","blocks":[{}]}}}}"#,
            blocks.join(",")
        )
        .into_bytes()
    }

    fn six_long_choices() -> Vec<u8> {
        let label = "A choice that is wordy enough to need more than a single line";
        let choices: Vec<String> = (1..=6)
            .map(|n| format!(r#"{{"id":"c{n}","label":"{n}. {label}"}}"#))
            .collect();
        format!(
            r#"{{"rev":13,"kind":"ask","status":{{"line":"","detail":""}},
            "ask":{{"ask_id":"many","question":"Which of these is it, and what should happen next?",
            "context":"Choose one","choices":[{}]}}}}"#,
            choices.join(",")
        )
        .into_bytes()
    }

    fn every_screen(context: &Context) -> Vec<(String, Screen)> {
        let mut screens = Vec::new();
        for (name, bytes) in [
            ("page", SCREEN_PAGE.to_vec()),
            ("ask", SCREEN_ASK.to_vec()),
            ("ask-six-long-choices", six_long_choices()),
            ("status", SCREEN_STATUS.to_vec()),
            ("long-page", long_page()),
        ] {
            let (mut app, poll) = paired();
            deliver(&mut app, poll, &bytes);
            screens.push((name.to_owned(), app.screen(context)));
            if name == "long-page" {
                let total = app.page_count(context);
                for page in 1..total {
                    app.page = page;
                    screens.push((format!("long-page-{page}"), app.screen(context)));
                }
            }
        }
        let (mut app, _) = paired();
        app.trouble = Some("The bridge is not answering. Check that it is running.".into());
        screens.push(("resting-with-trouble".to_owned(), app.screen(context)));
        app.address = "192.168.100.199:29331".to_owned();
        app.on_action(&mut Context::default(), action_id(REPAIR));
        assert_eq!(app.view, View::Address);
        screens.push(("address".to_owned(), app.screen(context)));
        app.view = View::Code;
        screens.push(("code".to_owned(), app.screen(context)));
        app.view = View::Pairing;
        screens.push(("pairing".to_owned(), app.screen(context)));
        app.view = View::Sending;
        screens.push(("sending".to_owned(), app.screen(context)));
        screens
    }

    #[test]
    fn every_screen_fits_the_panel_at_every_text_size() {
        let mut failures = Vec::new();
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Panel::default(), metrics).context();
            for (name, screen) in every_screen(&context) {
                let errors = screen
                    .diagnostics(&metrics, &Chrome::measuring(false))
                    .issues
                    .into_iter()
                    .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                    .map(|issue| format!("{:?}", issue.kind))
                    .collect::<Vec<_>>();
                if !errors.is_empty() {
                    failures.push(format!("{scale:?} {name}: {errors:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_first_run_asks_for_the_address_and_touches_no_network() {
        let mut app = Panel::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: None,
            },
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_none() && posted(&commands).is_none());
        assert_eq!(app.view, View::Address);
    }

    #[test]
    fn a_remembered_pairing_polls_with_its_token_and_no_rev_yet() {
        let (_, poll) = paired();
        let (mut app, _) = paired();
        let _ = poll;
        let mut context = Context::default();
        app.poll = None;
        app.start_poll(&mut context);
        let (_, url) = fetched(&context.take_commands()).expect("a poll");
        assert_eq!(url, "https://192.168.1.5:8473/v1/screen?wait=25");
    }

    #[test]
    fn a_page_is_drawn_and_the_next_poll_names_its_rev() {
        let (mut app, poll) = paired();
        let commands = deliver(&mut app, poll, SCREEN_PAGE);
        let screen = painted(&commands).expect("a page is drawn");
        let text = shown(&screen).join(" ");
        assert!(
            text.contains("Saturday") && text.contains("lunch at one"),
            "{text}"
        );
        let nap = slept(&commands).expect("a nap before the next poll");
        let mut context = Context::default();
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        let (_, url) = fetched(&context.take_commands()).expect("polls again");
        assert!(url.contains("rev=7&wait=25"), "{url}");
    }

    #[test]
    fn a_picture_is_fetched_by_its_blob_and_an_unreadable_one_is_said() {
        let (mut app, poll) = paired();
        let commands = deliver(
            &mut app,
            poll,
            br#"{"rev":4,"kind":"image","status":{"line":"","detail":""},
                "image":{"blob":"b9","fill":true}}"#,
        );
        let (task, url) = fetched(&commands).expect("the picture is asked for");
        assert_eq!(url, "https://192.168.1.5:8473/v1/blob/b9");
        let commands = deliver(&mut app, task, b"not a picture");
        let screen = painted(&commands).expect("the failure is drawn");
        assert!(shown(&screen)
            .iter()
            .any(|line| line.contains("could not open")));
        assert!(app.picture.is_none());
    }

    #[test]
    fn a_quiet_poll_draws_nothing() {
        let (mut app, poll) = paired();
        let commands = deliver(&mut app, poll, SCREEN_PAGE);
        let nap = slept(&commands).expect("nap");
        let mut context = Context::default();
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        let (next, _) = fetched(&context.take_commands()).expect("poll");
        let commands = deliver(&mut app, next, br#"{"rev":7,"unchanged":true}"#);
        assert!(painted(&commands).is_none());
    }

    #[test]
    fn a_resting_screen_says_what_muse_said() {
        let (mut app, poll) = paired();
        let commands = deliver(&mut app, poll, SCREEN_STATUS);
        let text = shown(&painted(&commands).expect("drawn")).join(" ");
        assert!(text.contains("Writing your brief") && text.contains("Back in a few minutes"));
    }

    #[test]
    fn back_puts_a_page_away_until_the_screen_changes() {
        let (mut app, poll) = paired();
        deliver(&mut app, poll, SCREEN_PAGE);
        let commands = act(&mut app, ActionId::BACK);
        let text = shown(&painted(&commands).expect("redrawn")).join(" ");
        assert!(
            text.contains("Waiting for Muse") || text.contains("Planning"),
            "{text}"
        );
        assert!(!text.contains("lunch at one"));
        let mut context = Context::default();
        app.poll = None;
        app.start_poll(&mut context);
        let (poll, _) = fetched(&context.take_commands()).expect("poll");
        let commands = deliver(&mut app, poll, SCREEN_ASK);
        assert!(shown(&painted(&commands).expect("the question"))
            .join(" ")
            .contains("Move lunch"));
    }

    #[test]
    fn a_long_page_is_split_without_losing_words() {
        let (mut app, poll) = paired();
        deliver(&mut app, poll, &long_page());
        let context =
            kobo_sdk::AppRunner::with_metrics(Panel::default(), CLARA_BW_METRICS).context();
        let pages = app.page_count(&context);
        assert!(pages > 1);
        let mut seen = String::new();
        for page in 0..pages {
            app.page = page;
            seen.push_str(&shown(&app.screen(&context)).join(" "));
            seen.push(' ');
        }
        for n in 0..14 {
            assert!(
                seen.contains(&format!("{n}. A paragraph")),
                "paragraph {n} is missing"
            );
        }
        let last = act(&mut app, action_id(NEXT));
        assert!(painted(&last).is_some());
    }

    #[test]
    fn tapping_an_answer_posts_it_once_and_the_next_poll_moves_on() {
        let (mut app, poll) = paired();
        deliver(&mut app, poll, SCREEN_ASK);
        let commands = act(&mut app, action_id("choose-1"));
        let (task, url, body) = posted(&commands).expect("the tap is posted");
        assert_eq!(url, "https://192.168.1.5:8473/v1/event");
        assert_eq!(body, r#"{"ask_id":"lunch","choice":"c2"}"#);
        assert_eq!(app.view, View::Sending);
        // A second tap while sending posts nothing.
        assert!(posted(&act(&mut app, action_id("choose-0"))).is_none());
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"{\"ok\":true}".to_vec()),
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "polling resumes");
        assert_eq!(app.view, View::Live);
        assert!(!shown(&painted(&commands).expect("drawn"))
            .join(" ")
            .contains("Move lunch"));
    }

    #[test]
    fn a_tap_the_bridge_refused_is_said_and_the_question_stays() {
        let (mut app, poll) = paired();
        deliver(&mut app, poll, SCREEN_ASK);
        let (task, _, _) = posted(&act(&mut app, action_id("choose-0"))).expect("posted");
        let mut context = Context::default();
        app.on_task(&mut context, task, TaskOutcome::Failed(TaskError::NotFound));
        let text = shown(&painted(&context.take_commands()).expect("drawn")).join(" ");
        assert!(
            text.contains("already moved on") && text.contains("Move lunch"),
            "{text}"
        );
    }

    #[test]
    fn a_dead_bridge_is_named_and_polling_resumes_after_a_nap() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(
            &mut context,
            poll,
            TaskOutcome::Failed(TaskError::Unreachable),
        );
        let commands = context.take_commands();
        assert!(painted(&commands).is_some() && slept(&commands).is_some());
        assert!(app.trouble.is_some());
    }

    #[test]
    fn local_time_follows_the_given_offset_and_rejects_an_invalid_one() {
        let utc = local_time(0).expect("system clock");
        let ahead = local_time(330).expect("system clock");
        let total = |(h, m): (u8, u8)| i32::from(h) * 60 + i32::from(m);
        let delta = (total(ahead) - total(utc)).rem_euclid(1440);
        assert!(
            delta == 330 || delta == 331 || delta == 329,
            "delta {delta}"
        );
        assert!(local_time(2000).is_none());
    }

    #[test]
    fn offsets_outside_the_clock_range_fall_back_to_labelled_utc() {
        assert_eq!(parse_offset("330"), Some(330));
        assert_eq!(parse_offset(" -480 "), Some(-480));
        assert_eq!(parse_offset("840"), Some(840));
        for bad in ["841", "-841", "30000", "abc", ""] {
            assert_eq!(parse_offset(bad), None, "{bad}");
        }
    }

    #[test]
    fn typed_code_characters_are_shown_in_capitals_as_they_are_sent() {
        let app = Panel {
            view: View::Code,
            keyboard: Keyboard::with_text("a3t".to_owned()),
            ..Panel::default()
        };
        let text = format!("{:?}", app.code_screen());
        // The on-screen keyboard is lowercase, so a capital can only be a box.
        for shown in ["\"A\"", "\"T\""] {
            assert!(text.contains(shown), "{shown}");
        }
    }

    #[test]
    fn an_empty_status_from_muse_reads_differently_from_no_contact_yet() {
        let (mut app, poll) = paired();
        assert!(format!("{:?}", app.resting()).contains("Waiting for Muse"));
        deliver(
            &mut app,
            poll,
            br#"{"rev":3,"kind":"status","status":{"line":"","detail":""},
                "page":null,"ask":null,"image":null}"#,
        );
        let text = format!("{:?}", app.resting());
        assert!(text.contains("Nothing new from Muse"));
        assert!(!text.contains("Waiting for Muse"));
    }

    #[test]
    fn the_update_time_is_set_only_by_a_validated_reply() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(
            &mut context,
            poll,
            TaskOutcome::Failed(TaskError::Unreachable),
        );
        assert!(app.updated.is_none());
        assert!(!format!("{:?}", app.resting()).contains("Updated "));

        let (mut app, poll) = paired();
        deliver(&mut app, poll, b"not json at all");
        assert!(app.updated.is_none());

        let (mut app, poll) = paired();
        app.offset_minutes = Some(0);
        deliver(&mut app, poll, SCREEN_STATUS);
        let (hour, minute) = app.updated.expect("a validated reply stamps the time");
        assert!(hour < 24 && minute < 60);

        assert!(format!("{:?}", app.resting()).contains("Updated "));

        let (mut app, poll) = paired();
        assert!(app.offset_minutes.is_none());
        deliver(&mut app, poll, SCREEN_STATUS);
        assert!(app.updated.is_some());
        let text = format!("{:?}", app.resting());
        assert!(text.contains(" UTC"), "an unknown offset is labelled UTC");
        let (mut app, poll) = paired();
        app.offset_minutes = Some(330);
        deliver(&mut app, poll, SCREEN_STATUS);
        assert!(!format!("{:?}", app.resting()).contains(" UTC"));
    }

    #[test]
    fn typing_the_address_and_code_pairs_and_remembers_the_token() {
        let mut app = Panel::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: None,
            },
        );
        let _ = context.take_commands();
        app.keyboard = Keyboard::with_text("192.168.1.9");
        act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Code);
        app.keyboard = Keyboard::with_text("qk3mzp");
        let commands = act(&mut app, action_id("kb.enter"));
        let (task, url, body) = posted(&commands).expect("the code is posted");
        assert_eq!(url, "https://192.168.1.9:8473/v1/pair");
        assert_eq!(body, r#"{"code":"QK3MZP"}"#);
        assert_eq!(app.view, View::Pairing);

        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"token":"newtok"}"#.to_vec()),
        );
        let commands = context.take_commands();
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) => Some((key.clone(), value.clone())),
            _ => None,
        });
        assert_eq!(
            saved,
            Some((PAIRED.to_owned(), b"192.168.1.9:8473\nnewtok".to_vec()))
        );
        assert!(!fetched(&commands).expect("polls").1.contains("newtok"));
        let sent = commands.iter().find_map(|command| match command {
            Command::Spawn {
                work: Task::Fetch { headers, .. },
                ..
            } => Some(headers.clone()),
            _ => None,
        });
        assert_eq!(
            sent,
            Some(vec![Header::new("X-Muse-Panel-Token", "newtok")])
        );
        assert_eq!(app.view, View::Live);
    }

    #[test]
    fn an_incomplete_code_is_not_sent() {
        let mut app = Panel {
            view: View::Code,
            address: "10.0.0.2:8473".into(),
            keyboard: Keyboard::with_text("ab1"),
            ..Panel::default()
        };
        assert!(posted(&act(&mut app, action_id("kb.enter"))).is_none());
        assert_eq!(app.view, View::Code);
    }

    #[test]
    fn a_wrong_code_returns_to_the_code_screen_with_the_reason() {
        let mut app = Panel {
            view: View::Pairing,
            address: "10.0.0.2:8473".into(),
            ..Panel::default()
        };
        let mut context = Context::default();
        app.pair = Some(TaskId(1));
        app.on_task(
            &mut context,
            TaskId(1),
            TaskOutcome::Failed(TaskError::NotFound),
        );
        assert_eq!(app.view, View::Code);
        assert!(app
            .trouble
            .as_deref()
            .unwrap_or("")
            .contains("did not work"));
    }
}
