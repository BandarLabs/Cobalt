//! The permission prompt, moved to the armchair.
//!
//! A coding agent on the desk stops to ask "may I run this?" and the asking
//! goes wherever this reader is. The sidekick daemon on the computer catches
//! the question through the agent's own hook system and holds it; this
//! application collects it over one long-polled fetch, prints the command in
//! full, and offers exactly three answers under a thumb: Allow, Deny, and
//! leave it for the terminal.
//!
//! The design rule is that the panel earns its repaints. An empty poll asks
//! again without drawing anything; a question repaints once and then the
//! panel holds it at zero power for as long as the decision takes, which is
//! the one thing this screen does better than the phone it replaces.
//!
//! Pairing is typed once and remembered: the daemon's address, then the
//! six-character code `kobo-sidekickd init` printed. The code rides every
//! request so nobody else on the network can watch the questions or answer
//! them, and the connection is TLS against a root the owner installed with
//! `kobo trust set`.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, ControlState, Failure, Glyph, KoboApp, Position,
    Screen, ScreenBuilder, Space, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const TITLE: &str = "Sidekick";
/// Where the address and code are remembered between sessions.
const PAIRED: &str = "paired";

const ALLOW: &str = "allow";
const DENY: &str = "deny";
const IGNORE: &str = "pass";
/// Sends the ticked answers to a question that takes more than one.
const SEND: &str = "send";
const REPAIR: &str = "repair";
const PREVIOUS: &str = "previous-page";
const NEXT: &str = "next-page";

/// The port `kobo-sidekick run` listens on, filled in when the owner types a
/// bare address, so the common case is typing one thing instead of two.
const DEFAULT_PORT: &str = "9331";

/// How many characters `kobo-sidekickd init` puts in a pairing code. The
/// code screen draws this many boxes and refuses a seventh character.
const CODE_LENGTH: usize = 6;

/// How long the daemon holds an empty poll before answering "nothing yet".
/// Well under the runtime's own request ceiling, so a quiet afternoon is a
/// steady heartbeat of short requests rather than a stack of timeouts.
const POLL_WAIT: &str = "25";

/// A question is a command line and change; a reply is smaller.
const MAX_REPLY: u32 = 16 * 1024;

/// How long to sleep after a failed poll before trying again. Long enough
/// not to spin on a dead network, short enough that the daemon coming back
/// is noticed before anyone walks over to check.
const NAP_SECONDS: u32 = 10;

/// How long to wait after an answer that was not a question before asking
/// again.
///
/// The daemon holds a poll open for [`POLL_WAIT`] seconds when nothing is
/// waiting, so this costs nothing in the ordinary case. It matters in the two
/// where the daemon answers straight away: an older daemon that does not hold
/// the request, and a board of questions, which the daemon can answer as fast
/// as it is asked. Either way the loop ran flat out -- three hundred and sixty
/// requests in ten seconds with the panel sitting still -- on a device whose
/// radio is the largest single draw on its battery.
const BETWEEN_POLLS: u32 = 2;

/// The most characters of a command drawn on the panel. Enough to read
/// almost any real command whole; past this the reader should be at the
/// terminal anyway, and the tail is marked rather than silently missing.
const MAX_DETAIL: usize = 600;

/// One answer offered by name, drawn as a row of its own.
#[derive(Clone, Debug, PartialEq)]
struct Choice {
    label: String,
    description: String,
}

/// One question, as the daemon sent it.
#[derive(Clone, Debug, PartialEq)]
struct Ask {
    id: u32,
    source: String,
    session: String,
    tool: String,
    detail: String,
    /// The answers this question brought with it. Empty is the usual case.
    choices: Vec<Choice>,
    /// Whether allow and deny mean anything here. A multiple-choice
    /// question is not a permission, so allowing it would answer nothing.
    permission: bool,
    /// Whether more than one choice may be taken. A tap ticks rather than
    /// answers, and the answer is sent by a button of its own.
    multi: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    /// Waiting for the store to say whether a pairing exists.
    #[default]
    Opening,
    /// Typing the daemon's address.
    Address,
    /// Typing the pairing code.
    Code,
    /// Paired, polling, nothing to decide.
    Watching,
    /// More than one terminal is waiting; choose the question before
    /// deciding it so one terminal cannot answer another's prompt.
    Board,
    /// A question is on the panel.
    Asking,
    /// An answer is on its way to the daemon.
    Sending,
}

#[derive(Clone, Default)]
struct QuestionPage {
    detail: String,
    choices: Vec<usize>,
    /// A single oversized choice, continued without changing its answer label.
    /// Its stable option number identifies every part, even for a long name.
    choice_text: Option<String>,
}

#[derive(Default)]
struct Sidekick {
    view: View,
    keyboard: Keyboard,
    /// The daemon, as `host:port`.
    address: String,
    /// The pairing code, sent as the token on every request.
    code: String,
    /// The question on the panel, while there is one.
    ask: Option<Ask>,
    /// The current daemon snapshot. It is deliberately display-only: the
    /// daemon remains the sole owner of questions and their answers.
    board: Vec<Ask>,
    /// Which choices are ticked, for a question that takes more than one.
    /// Cleared with every new question rather than carried between them.
    ticked: Vec<bool>,
    poll: Option<TaskId>,
    answer: Option<TaskId>,
    nap: Option<TaskId>,
    /// The last decision, stated on the watching screen so a glance says
    /// the tap counted even after the question has left the panel.
    last: Option<String>,
    trouble: Option<String>,
    page: usize,
}

impl Sidekick {
    fn show(&self, context: &mut Context) {
        // Back retreats one step inside the flow -- code to address, and a
        // question to "leave it for the terminal" -- rather than leaving.
        let owns_back = matches!(self.view, View::Code | View::Asking);
        context.set_screen(self.screen(context).with_own_back(owns_back));
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("sidekick-opening")
                .top_bar(TITLE)
                .activity("Opening", None)
                .build(),
            View::Address => self.address_screen(),
            View::Code => self.code_screen(),
            View::Watching => self.watching(),
            View::Board => self.board(context),
            View::Asking => self.asking(context),
            View::Sending => ScreenBuilder::new("sidekick-sending")
                .top_bar(TITLE)
                .activity("Sending your answer", None)
                .build(),
        }
    }

    /// Step one of pairing: where the daemon is.
    fn address_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("sidekick-address")
            .top_bar(TITLE)
            // A step's heading rather than a screen's: a keyboard takes half
            // the panel, and the display size wrapped to two lines at the
            // largest text setting and pushed the field off it.
            .heading_at_level(2, "Pair with your computer")
            // The command, by name. "Open Sidekick in the Cobalt desktop app"
            // is true and useless: the thing that has to happen is that a
            // daemon is running on the computer, and this is what starts it
            // and prints the two things this screen is about to ask for.
            .text("On your computer, run kobo sidekick init. It prints an address and a code.");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .field("address.box", self.keyboard.text(), "192.168.1.20:9331")
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Next")
            .build()
    }

    /// Step two: the code that proves the reader is the owner's.
    fn code_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("sidekick-code")
            .top_bar(TITLE)
            .heading_at_level(2, "Now the pairing code")
            // Short on purpose. The address is as long as somebody's network
            // makes it, and a sentence built around it ran off the panel at
            // the larger text sizes with a keyboard already taking the bottom
            // half: the renderer refused the screen and pairing stopped dead.
            ;
        screen = if let Some(trouble) = &self.trouble {
            screen.banner(BannerLevel::Attention, trouble.clone())
        } else {
            screen.text("The six characters shown beside the address.")
        };
        let typed: Vec<char> = self.keyboard.text().trim().chars().collect();
        let boxes = (0..CODE_LENGTH).map(|slot| {
            (
                format!("code.{slot}"),
                typed.get(slot).map(char::to_string).unwrap_or_default(),
            )
        });
        screen
            .grid(6, true, boxes)
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Pair")
            .build()
    }

    /// Paired and quiet. Painted when the state changes, never per poll.
    fn watching(&self) -> Screen {
        // The splash carries its sentence only while it is the whole screen.
        // With a last answer and a pairing under it as well, the four lines
        // of subtitle at 170% left the rest of the panel with less room than
        // its own words needed, and the renderer refused the screen: a reader
        // who had just answered a question saw nothing at all.
        let mut screen = ScreenBuilder::new("sidekick-watching")
            .top_bar(TITLE)
            .splash(
                Some(Glyph::Circle),
                "Watching",
                if self.last.is_some() {
                    String::new()
                } else {
                    "Questions from your coding agents appear here the moment they ask.".to_owned()
                },
            );
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }

        if let Some(last) = &self.last {
            screen = screen.section("Last answer").text(last.clone());
        }
        screen
            .section("Paired with")
            .text(self.address.clone())
            .spacer(Space::Small)
            .button(REPAIR, "Change pairing")
            .build()
    }

    /// Every waiting terminal stays reachable, measured below the actual header.
    fn board_prefix(&self) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("sidekick-board")
            .top_bar(TITLE)
            .heading("Waiting questions")
            .text("Choose a terminal to answer.");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
    }

    fn board_rows(&self) -> Vec<(String, String)> {
        self.board
            .iter()
            .map(|ask| {
                (
                    format!("{}{}", agent_name(&ask.source), session_suffix(ask)),
                    format!("{} · {}", ask.tool, trimmed_to(&ask.detail, 72)),
                )
            })
            .collect()
    }

    fn board_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.board_rows();
        let rows = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect::<Vec<_>>();
        context.paginate_rows_under(
            &rows,
            false,
            Position::AtTheFoot,
            &self.board_prefix().build(),
        )
    }

    fn page_controls(screen: ScreenBuilder, page: usize, pages: usize) -> ScreenBuilder {
        if pages <= 1 {
            return screen;
        }
        screen.page_turns(PREVIOUS, NEXT).page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages).unwrap_or(u16::MAX),
        )
    }

    fn board(&self, context: &Context) -> Screen {
        let rows = self.board_rows();
        let pages = self.board_pages(context);
        let page = self.page.min(pages.len().saturating_sub(1));
        let indices = pages.get(page).cloned().unwrap_or_default();
        let screen = self.board_prefix().rows(indices.into_iter().map(|index| {
            let (title, summary) = &rows[index];
            (
                board_action(index),
                title.clone(),
                summary.clone(),
                Glyph::Chat,
            )
        }));
        Self::page_controls(screen, page, pages.len()).build()
    }

    /// Compose the exact page, including fixed-height decision controls. The
    /// fill keeps the actions in the same place while question pages turn.
    fn question_screen(
        &self,
        ask: &Ask,
        content: &QuestionPage,
        page: usize,
        pages: usize,
    ) -> Screen {
        let mut screen = ScreenBuilder::new("sidekick-asking")
            .top_bar(TITLE)
            .heading(format!("{} asks", agent_name(&ask.source)))
            .byline(0, self.asked_by(ask));
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if !content.detail.is_empty() {
            screen = screen.quote(0, content.detail.clone());
        }
        if !content.choices.is_empty() {
            screen = screen.rows(content.choices.iter().map(|&index| {
                let choice = &ask.choices[index];
                (
                    chosen_action(index),
                    if content.choice_text.is_some() {
                        format!("Option {}", index + 1)
                    } else {
                        choice.label.clone()
                    },
                    content
                        .choice_text
                        .clone()
                        .unwrap_or_else(|| choice.description.clone()),
                    if self.is_ticked(index) {
                        Glyph::Check
                    } else {
                        Glyph::Circle
                    },
                )
            }));
        }
        screen = screen.fill();
        if ask.multi {
            let state = if self.ticked.iter().any(|ticked| *ticked) {
                ControlState::Enabled
            } else {
                ControlState::Disabled
            };
            screen = screen.button_with_state(SEND, "Send these answers", state);
        }
        if ask.permission {
            screen = screen.buttons([(ALLOW, "Allow"), (DENY, "Deny")]);
        }
        // Pinned rather than in the flow, so it is in the same place on every
        // page of a long question: a page packed to the last line used to
        // push it a few pixels lower than the page after it.
        screen = screen.bottom_action(IGNORE, "Leave it for the terminal");
        Self::page_controls(screen, page, pages).build()
    }

    /// Pack against the renderer, never a guessed character or row budget.
    /// Text chunks retain every character (including command whitespace).
    fn question_pages(&self, context: &Context, ask: &Ask) -> Vec<QuestionPage> {
        let fits = |page: &QuestionPage| {
            !self
                .question_screen(ask, page, 0, 2)
                .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        };
        let detail = trimmed(&ask.detail);
        let mut rest = detail.as_str();
        let mut pages = Vec::new();
        while !rest.is_empty() {
            let end = fitting_prefix(rest, |text| {
                fits(&QuestionPage {
                    detail: text.to_owned(),
                    ..QuestionPage::default()
                })
            });
            pages.push(QuestionPage {
                detail: rest[..end].to_owned(),
                ..QuestionPage::default()
            });
            rest = &rest[end..];
        }
        if pages.is_empty() {
            pages.push(QuestionPage::default());
        }
        for index in 0..ask.choices.len() {
            let last = pages.last_mut().expect("at least one page");
            let mut candidate = last.clone();
            candidate.choices.push(index);
            if last.choice_text.is_none() && fits(&candidate) {
                *last = candidate;
                continue;
            }
            let mut alone = QuestionPage {
                choices: vec![index],
                ..QuestionPage::default()
            };
            if last.detail.is_empty() && last.choices.is_empty() {
                pages.pop();
            }
            if fits(&alone) {
                pages.push(alone);
                continue;
            }
            // Moving an oversized row to a fresh page does not make it fit.
            // Read its entire name and description across numbered parts;
            // each part still selects the original choice, never a fragment.
            let choice = &ask.choices[index];
            let text = if choice.description.is_empty() {
                choice.label.clone()
            } else {
                format!("{}\n\n{}", choice.label, choice.description)
            };
            let mut rest = text.as_str();
            while !rest.is_empty() {
                let end = fitting_prefix(rest, |text| {
                    alone.choice_text = Some(text.to_owned());
                    fits(&alone)
                });
                alone.choice_text = Some(rest[..end].to_owned());
                pages.push(alone.clone());
                rest = &rest[end..];
            }
        }
        pages
    }

    fn asking(&self, context: &Context) -> Screen {
        let Some(ask) = &self.ask else {
            return self.watching();
        };
        let pages = self.question_pages(context, ask);
        let page = self.page.min(pages.len().saturating_sub(1));
        self.question_screen(ask, &pages[page], page, pages.len())
    }

    /// Who is asking: the tool, the terminal it is running in, and the
    /// computer this reader is paired with.
    fn asked_by(&self, ask: &Ask) -> String {
        let mut line = ask.tool.clone();
        let session = session_suffix(ask);
        if !session.is_empty() {
            line.push_str(&session);
        }
        if !self.address.is_empty() {
            line.push_str(" \u{b7} ");
            line.push_str(&self.address);
        }
        line
    }

    /// Starts the next long poll. One in flight at a time, always.
    fn poll(&mut self, context: &mut Context) {
        if self.poll.is_some() {
            return;
        }
        let url = format!(
            "https://{}/pending?token={}&all=true&wait={POLL_WAIT}",
            self.address, self.code
        );
        self.poll = context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY,
            credential: None,
            headers: Vec::new(),
        });
    }

    /// Sends the tapped decision back to the daemon.
    fn decide(&mut self, context: &mut Context, choice: &str) {
        self.answer_daemon(context, choice, Vec::new());
    }

    /// Sends back the question's own answers, by the labels they came with.
    /// The daemon does not interpret them, and neither does this.
    fn choose(&mut self, context: &mut Context, labels: Vec<String>) {
        self.answer_daemon(context, "", labels);
    }

    /// Whether the choice at `index` has been ticked.
    fn is_ticked(&self, index: usize) -> bool {
        self.ticked.get(index).copied().unwrap_or(false)
    }

    /// Every ticked label, in the order the agent offered them.
    fn ticked_labels(&self) -> Vec<String> {
        self.ask
            .iter()
            .flat_map(|ask| ask.choices.iter())
            .enumerate()
            .filter(|(index, _)| self.is_ticked(*index))
            .map(|(_, choice)| choice.label.clone())
            .collect()
    }

    fn answer_daemon(&mut self, context: &mut Context, choice: &str, labels: Vec<String>) {
        let Some(ask) = &self.ask else {
            return;
        };
        let sentence = if labels.is_empty() {
            format!(
                "{} {} for {}.",
                decided(choice),
                trimmed_to(&ask.detail, 60),
                agent_name(&ask.source)
            )
        } else {
            format!(
                "Answered {} for {}.",
                labels.join(", "),
                agent_name(&ask.source)
            )
        };
        let body = kobo_json::ObjectBuilder::new()
            .set("token", self.code.as_str())
            .set("id", ask.id)
            .set("choice", choice)
            .set(
                "labels",
                kobo_json::Value::Array(labels.into_iter().map(kobo_json::Value::String).collect()),
            )
            .build()
            .to_json();
        let work = Task::Post {
            url: format!("https://{}/answer", self.address),
            body,
            content_type: "application/json".to_owned(),
            credential: None,
            headers: Vec::new(),
            max_bytes: MAX_REPLY,
        };
        if let Some(task) = context.spawn(work) {
            self.answer = Some(task);
            // Cut to what the panel will actually hold rather than to a
            // character count: sixty characters is a line and a half at the
            // default text size and four lines at 170%, where the renderer
            // refused the whole screen and a reader who had just answered
            // saw nothing at all.
            self.last = Some(context.clamped_row(&sentence, 2, false));
            self.view = View::Sending;
            self.trouble = None;
            self.show(context);
        } else {
            self.trouble = Some("Still sending the last answer.".to_owned());
            self.show(context);
        }
    }

    /// What came back from a poll.
    fn on_poll(&mut self, context: &mut Context, outcome: TaskOutcome) {
        if !matches!(self.view, View::Watching | View::Board) {
            // The pairing screens are up mid-poll. Nothing is shown over the
            // typing and nothing spins the loop; a question stays queued on
            // its daemon, for the next poll of whatever pairing wins.
            return;
        }
        match outcome {
            TaskOutcome::Completed(bytes) => {
                let repaint = self.trouble.take().is_some();
                let asks = read_asks(&bytes);
                if asks.len() == 1 {
                    let ask = asks.into_iter().next().expect("one ask");
                    self.ticked = vec![false; ask.choices.len()];
                    self.ask = Some(ask);
                    self.page = 0;
                    self.view = View::Asking;
                    self.show(context);
                    // Deliberately no next poll: the daemon queues anything
                    // else that arrives until this question is decided.
                    return;
                }
                if asks.len() > 1 {
                    if self.board != asks {
                        self.page = 0;
                    }
                    self.board = asks;
                    self.view = View::Board;
                    self.show(context);
                    // The board is refreshed, not watched: the daemon answers
                    // at once while questions are waiting, so asking again
                    // immediately is a loop with a picture of a list on it.
                    self.nap = context.spawn(Task::Sleep {
                        seconds: BETWEEN_POLLS,
                    });
                    return;
                }
                if repaint {
                    self.show(context);
                }
                // Nothing waiting. Asking again is right; asking again this
                // instant is a loop.
                self.nap = context.spawn(Task::Sleep {
                    seconds: BETWEEN_POLLS,
                });
            }
            TaskOutcome::Failed(error) => {
                self.trouble = Some(Failure::of(error).advice.to_owned());
                self.show(context);
                self.nap = context.spawn(Task::Sleep {
                    seconds: NAP_SECONDS,
                });
            }
            // The runtime withdrew the task; on_foreground restarts the loop.
            TaskOutcome::Cancelled => {}
        }
    }

    /// What came back from posting an answer.
    fn on_answer(&mut self, context: &mut Context, outcome: &TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(bytes) => {
                if !answer_landed(bytes) {
                    // The daemon said no: the question was gone -- timed out
                    // or already collected -- before the tap arrived. Saying
                    // "Allowed" now would be claiming a decision nobody got.
                    self.last = Some("That question was gone before the answer arrived.".into());
                }
                self.ask = None;
                self.view = View::Watching;
                self.trouble = None;
                self.show(context);
                self.poll(context);
            }
            TaskOutcome::Failed(error) => {
                // The question is still on the daemon's board, so it comes
                // back to the panel with the three answers intact.
                self.view = View::Asking;
                self.last = None;
                self.trouble = Some(Failure::of(*error).advice.to_owned());
                self.show(context);
            }
            TaskOutcome::Cancelled => {
                self.view = View::Asking;
                self.last = None;
                self.show(context);
            }
        }
    }

    /// A submitted address, cleaned and given the default port if none was
    /// typed. `None` means it could not be an address, with the reason left
    /// in `trouble`.
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
            self.trouble = Some("Enter the address shown on your computer.".to_owned());
            None
        }
    }

    /// Handles a tap while a keyboard is up. Returns whether it was one.
    fn typing(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(pressed) = self.keyboard.press(action) else {
            return false;
        };
        match pressed {
            Pressed::Edited | Pressed::Shifted => {
                // A seventh character has nowhere to be drawn, and a code
                // longer than the boxes would be an entry the panel disagrees
                // with. Refused at the keyboard, not trimmed at submission.
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
                    let code = self.keyboard.text().trim().to_owned();
                    if code.chars().count() != CODE_LENGTH {
                        self.trouble = Some("Enter all six characters.".to_owned());
                        self.show(context);
                        return true;
                    }
                    self.code = code;
                    self.keyboard.clear();
                    let record = format!("{}\n{}", self.address, self.code);
                    context.store().save(PAIRED, record.into_bytes());
                    self.view = View::Watching;
                    self.trouble = None;
                    // A poll still in flight belongs to the old pairing.
                    // Forgetting its id makes whatever it brings back land on
                    // nothing, and clears the way for this pairing's poll.
                    self.poll = None;
                    self.show(context);
                    self.poll(context);
                }
                _ => {}
            },
        }
        true
    }
}

/// Keep UTF-8 and whitespace intact while preferring a word boundary. Both
/// callers measure the complete screen, including the controls and page turns.
fn fitting_prefix(text: &str, mut fits: impl FnMut(&str) -> bool) -> usize {
    let ends = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let (mut low, mut high) = (0, ends.len() - 1);
    while low < high {
        let mid = (low + high + 1) / 2;
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

/// The agent's name as a person says it, not as its process does.
fn agent_name(source: &str) -> &str {
    match source {
        "codex" => "Codex",
        "claude" => "Claude Code",
        other => other,
    }
}

/// The verb for the watching screen's "last answer" line.
fn decided(choice: &str) -> &'static str {
    match choice {
        ALLOW => "Allowed",
        DENY => "Denied",
        _ => "Left at the terminal:",
    }
}

fn trimmed(detail: &str) -> String {
    trimmed_to(detail, MAX_DETAIL)
}

/// The front of a long command, with the cut marked. Counted in characters
/// rather than bytes so the mark never splits one in half.
fn trimmed_to(detail: &str, most: usize) -> String {
    if detail.chars().count() <= most {
        return detail.to_owned();
    }
    let mut kept: String = detail.chars().take(most).collect();
    kept.push('…');
    kept
}

/// Whether the daemon said the tap landed on a live question. Anything but
/// a clear yes is a no: an unreadable reply gets the same caution.
fn answer_landed(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| kobo_json::parse(text).ok())
        .and_then(|body| body.get("ok").and_then(kobo_json::Value::as_bool))
        == Some(true)
}

/// The question in a poll's body, if the body carries one.
fn read_ask(bytes: &[u8]) -> Option<Ask> {
    let text = std::str::from_utf8(bytes).ok()?;
    let body = kobo_json::parse(text).ok()?;
    let ask = body.get("ask").unwrap_or(&body);
    let id = u32::try_from(ask.get("id").and_then(kobo_json::Value::as_i64)?).ok()?;
    let field = |name: &str| {
        ask.get(name)
            .and_then(kobo_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let choices = ask
        .get("choices")
        .and_then(kobo_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let text = |name: &str| {
                        item.get(name)
                            .and_then(kobo_json::Value::as_str)
                            .unwrap_or("")
                            .to_owned()
                    };
                    let label = text("label");
                    (!label.is_empty()).then(|| Choice {
                        label,
                        description: text("description"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Ask {
        id,
        source: field("source"),
        session: field("session"),
        tool: field("tool"),
        detail: field("detail"),
        choices,
        // Absent means a permission, which is what almost every ask is.
        permission: ask.get("permission").and_then(kobo_json::Value::as_bool) != Some(false),
        multi: ask.get("multi").and_then(kobo_json::Value::as_bool) == Some(true),
    })
}

/// The new board envelope, while accepting the old single-question response
/// during daemon upgrades.
fn read_asks(bytes: &[u8]) -> Vec<Ask> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Ok(body) = kobo_json::parse(text) else {
        return Vec::new();
    };
    let items = body
        .get("asks")
        .and_then(kobo_json::Value::as_array)
        .map(<[kobo_json::Value]>::to_vec)
        .unwrap_or_default();
    if !items.is_empty() {
        return items
            .into_iter()
            .filter_map(|ask| read_ask(&ask.to_json().into_bytes()))
            .collect();
    }
    read_ask(bytes).into_iter().collect()
}

fn board_action(index: usize) -> String {
    format!("board.{index}")
}

fn session_suffix(ask: &Ask) -> String {
    if ask.session.is_empty() {
        String::new()
    } else {
        format!(" · {}", ask.session)
    }
}

/// The action name for the nth offered answer. Positional because a label
/// is the agent's words and can be anything at all, including the name of
/// a control this screen already has.
fn chosen_action(index: usize) -> String {
    format!("choice.{index}")
}

impl KoboApp for Sidekick {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(PAIRED);
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
                // Both halves are trimmed. A pairing written by hand, or by any
                // editor that ends a file with a newline, otherwise carries that
                // newline into the code, and every poll comes back refused. The
                // daemon answers a wrong code with 403, kobo-net reads any 4xx
                // as nothing found, and the panel says the service had nothing
                // to return: an invisible whitespace reads as an idle server.
                let (address, code) = text.split_once('\n')?;
                Some((address.trim().to_owned(), code.trim().to_owned()))
            })
            .filter(|(address, code)| !address.is_empty() && !code.is_empty());
        if let Some((address, code)) = remembered {
            self.address = address;
            self.code = code;
            self.view = View::Watching;
            self.show(context);
            self.poll(context);
        } else {
            self.view = View::Address;
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            match self.view {
                // A question dismissed is a question left for the terminal,
                // said out loud rather than left dangling on the daemon.
                View::Asking => self.decide(context, IGNORE),
                View::Code => {
                    self.keyboard = Keyboard::with_text(&self.address);
                    self.view = View::Address;
                    self.trouble = None;
                    self.show(context);
                }
                _ => {}
            }
            return;
        }
        if matches!(self.view, View::Address | View::Code) && self.typing(context, action) {
            return;
        }
        if action == action_id(REPAIR) && self.view == View::Watching {
            self.keyboard = Keyboard::with_text(&self.address);
            self.view = View::Address;
            self.show(context);
            return;
        }
        if matches!(self.view, View::Board | View::Asking)
            && (action == action_id(PREVIOUS) || action == action_id(NEXT))
        {
            let pages = if self.view == View::Board {
                self.board_pages(context).len()
            } else {
                self.ask
                    .as_ref()
                    .map_or(1, |ask| self.question_pages(context, ask).len())
            };
            let current = self.page.min(pages.saturating_sub(1));
            self.page = if action == action_id(PREVIOUS) {
                current.saturating_sub(1)
            } else {
                current.saturating_add(1).min(pages.saturating_sub(1))
            };
            self.show(context);
            return;
        }
        if self.view == View::Board {
            if let Some(index) =
                (0..self.board.len()).find(|index| action == action_id(&board_action(*index)))
            {
                let ask = self.board[index].clone();
                self.ticked = vec![false; ask.choices.len()];
                self.ask = Some(ask);
                self.page = 0;
                self.view = View::Asking;
                self.show(context);
            }
            return;
        }
        if self.view == View::Asking {
            for choice in [ALLOW, DENY, IGNORE] {
                if action == action_id(choice) {
                    self.decide(context, choice);
                    return;
                }
            }
            if action == action_id(SEND) {
                let ticked = self.ticked_labels();
                if !ticked.is_empty() {
                    self.choose(context, ticked);
                }
                return;
            }
            let labels: Vec<String> = self
                .ask
                .iter()
                .flat_map(|ask| ask.choices.iter().map(|choice| choice.label.clone()))
                .collect();
            let multi = self.ask.as_ref().is_some_and(|ask| ask.multi);
            for (index, label) in labels.iter().enumerate() {
                if action != action_id(&chosen_action(index)) {
                    continue;
                }
                if multi {
                    // A tick is a change worth a repaint: without it there
                    // is no sign on the panel that the tap landed.
                    if let Some(slot) = self.ticked.get_mut(index) {
                        *slot = !*slot;
                    }
                    self.show(context);
                } else {
                    self.choose(context, vec![label.clone()]);
                }
                return;
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.poll == Some(task) {
            self.poll = None;
            self.on_poll(context, outcome);
        } else if self.answer == Some(task) {
            self.answer = None;
            self.on_answer(context, &outcome);
        } else if self.nap == Some(task) {
            self.nap = None;
            if matches!(self.view, View::Watching | View::Board) {
                self.poll(context);
            }
        }
    }

    fn on_foreground(&mut self, context: &mut Context) {
        // Whatever was in flight may have been cancelled while the panel was
        // elsewhere; a watching screen with no poll is a dead remote.
        if self.view == View::Watching && self.poll.is_none() && self.nap.is_none() {
            self.poll(context);
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("sidekick", Sidekick::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sidekick: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Sidekick, View, ALLOW, DENY, IGNORE, PAIRED, REPAIR, SEND};
    use kobo_sdk::keyboard::Keyboard;
    use kobo_sdk::{
        action_id, ActionId, Command, Context, KoboApp, Screen, StoreRequest, StoreResult, Task,
        TaskId, TaskOutcome,
    };
    use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, TextScale, CLARA_BW_METRICS};

    /// Every screen this application draws, with something on each of them.
    fn every_screen(context: &Context) -> Vec<(String, Screen)> {
        let (mut app, _) = paired();
        let mut screens = vec![("watching".to_owned(), app.screen(context))];
        // And the same screen after an answer, which is the one that has
        // something under the splash as well as beside it.
        app.last = Some("Allowed rm -rf target && cargo build --release for Claude Code.".into());
        screens.push(("watching-after-an-answer".to_owned(), app.screen(context)));
        // As long as somebody's network makes it. A screen built around a
        // short address in a test is a screen that fits only in the test.
        app.address = "192.168.100.199:29331".to_owned();
        app.view = View::Address;
        screens.push(("address".to_owned(), app.screen(context)));
        app.view = View::Code;
        screens.push(("code".to_owned(), app.screen(context)));
        app.view = View::Sending;
        screens.push(("sending".to_owned(), app.screen(context)));
        let mut asking = paired().0;
        asking.on_task(
            &mut Context::default(),
            asking.poll.expect("a poll"),
            question(
                1,
                "rm -rf ~/src/project/target && cargo build --release --locked",
            ),
        );
        screens.push(("asking".to_owned(), asking.screen(context)));
        let mut board = paired().0;
        board.on_task(
            &mut Context::default(),
            board.poll.expect("a poll"),
            fleet(),
        );
        screens.push(("board".to_owned(), board.screen(context)));
        screens
    }

    #[test]
    fn every_screen_fits_the_panel_at_every_text_size() {
        // The pairing screens are the ones at risk: a heading, a paragraph and
        // a raised keyboard fit at the default size and are refused outright
        // at the largest, which leaves a reader who has turned the type up
        // with a blank panel and no way to pair at all.
        let mut failures = Vec::new();
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Sidekick::default(), metrics).context();
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

    fn act(app: &mut Sidekick, action: ActionId) -> Vec<Command> {
        let mut context = Context::default();
        app.on_action(&mut context, action);
        context.take_commands()
    }

    /// A sidekick already paired and watching, with its first poll in flight.
    fn paired() -> (Sidekick, TaskId) {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:9331\nabc123".to_vec()),
            },
        );
        let poll = fetched(&context.take_commands()).expect("a poll starts").0;
        (app, poll)
    }

    /// The one `Task::Fetch` in a batch, as `(task, url)`.
    fn fetched(commands: &[Command]) -> Option<(TaskId, String)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Fetch { url, .. },
            } => Some((*task, url.clone())),
            _ => None,
        })
    }

    /// The one `Task::Post` in a batch, as `(task, url, body)`.
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

    /// Every line of text the panel would show, in order.
    fn shown(screen: &Screen) -> Vec<String> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    /// Two terminals asking at once, which is what a fleet looks like.
    fn fleet() -> TaskOutcome {
        TaskOutcome::Completed(
            r#"{"version":"4","asks":[
                {"id":1,"source":"claude","session":"cobalt ab12","tool":"Bash","detail":"cargo test --workspace"},
                {"id":2,"source":"codex","session":"notes cd34","tool":"shell","detail":"git push --force-with-lease"}
            ]}"#
            .as_bytes()
            .to_vec(),
        )
    }

    /// One question from a terminal that names itself.
    fn one_of_the_fleet() -> TaskOutcome {
        TaskOutcome::Completed(
            r#"{"ask":{"id":1,"source":"claude","session":"cobalt ab12","tool":"Bash",
                "detail":"cargo test --workspace"}}"#
                .as_bytes()
                .to_vec(),
        )
    }

    fn question(id: u32, detail: &str) -> TaskOutcome {
        TaskOutcome::Completed(
            format!(
                r#"{{"ask":{{"id":{id},"source":"codex","tool":"shell","detail":"{detail}"}}}}"#
            )
            .into_bytes(),
        )
    }

    /// A question that brought its own answers, as `AskUserQuestion` does.
    fn multiple_choice(id: u32) -> TaskOutcome {
        TaskOutcome::Completed(
            format!(
                r#"{{"ask":{{"id":{id},"source":"claude","tool":"Detail",
                "detail":"How much detail do you want?","permission":false,"choices":[
                {{"label":"Summary","description":"The short version"}},
                {{"label":"Every step","description":"Nothing left out"}}]}}}}"#
            )
            .into_bytes(),
        )
    }

    #[test]
    fn a_question_with_its_own_answers_shows_them_instead_of_allow_and_deny() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, multiple_choice(7));
        let screen = painted(&context.take_commands()).expect("the question was painted");
        let lines = shown(&screen);
        for words in [
            "How much detail do you want?",
            "Summary",
            "The short version",
            "Every step",
            "Nothing left out",
            // Still offered, because nobody should be trapped on the panel.
            "Leave it for the terminal",
        ] {
            assert!(
                lines.iter().any(|line| line.contains(words)),
                "no {words} on the panel: {lines:?}"
            );
        }
        // Allowing a multiple-choice question would answer nothing.
        for absent in ["Allow", "Deny"] {
            assert!(
                !lines.iter().any(|line| line.trim() == absent),
                "{absent} offered for a question that is not a permission: {lines:?}"
            );
        }
    }

    /// A question that takes more than one answer, as `multiSelect` does.
    fn multi_select(id: u32) -> TaskOutcome {
        TaskOutcome::Completed(
            format!(
                r#"{{"ask":{{"id":{id},"source":"claude","tool":"Sections",
                "detail":"Which sections should I include?","permission":false,"multi":true,
                "choices":[
                {{"label":"Introduction","description":"Opening context"}},
                {{"label":"Middle","description":"The argument"}},
                {{"label":"Conclusion","description":"Final summary"}}]}}}}"#
            )
            .into_bytes(),
        )
    }

    #[test]
    fn a_question_taking_several_answers_ticks_rather_than_sending_at_a_tap() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, multi_select(3));
        let _ = context.take_commands();
        // The first tap ticks and paints. It must not answer: there may be
        // more to say.
        let commands = act(&mut app, action_id("choice.0"));
        assert!(
            posted(&commands).is_none(),
            "a tick answered the question on its own"
        );
        assert!(
            painted(&commands).is_some(),
            "a tick left the panel with no sign the tap landed"
        );
        let commands = act(&mut app, action_id("choice.2"));
        assert!(posted(&commands).is_none(), "a second tick answered");
        // Now send both, in the order they were offered rather than tapped.
        let commands = act(&mut app, action_id(SEND));
        let (_, _, body) = posted(&commands).expect("the answers were sent");
        assert!(
            body.contains(r#""labels":["Introduction","Conclusion"]"#),
            "{body}"
        );
    }

    #[test]
    fn a_tick_taken_back_is_not_sent() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, multi_select(3));
        let _ = context.take_commands();
        let _ = act(&mut app, action_id("choice.1"));
        let _ = act(&mut app, action_id("choice.0"));
        // Second tap on the same row unticks it.
        let _ = act(&mut app, action_id("choice.1"));
        let commands = act(&mut app, action_id(SEND));
        let (_, _, body) = posted(&commands).expect("the answers were sent");
        assert!(body.contains(r#""labels":["Introduction"]"#), "{body}");
    }

    #[test]
    fn sending_nothing_is_not_an_answer() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, multi_select(3));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(SEND));
        assert!(
            posted(&commands).is_none(),
            "an empty answer went to the daemon"
        );
    }

    #[test]
    fn ticks_do_not_carry_from_one_question_to_the_next() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, multi_select(3));
        let _ = context.take_commands();
        let _ = act(&mut app, action_id("choice.0"));
        let commands = act(&mut app, action_id(SEND));
        let (task, _, _) = posted(&commands).expect("the answers were sent");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":true}"#.to_vec()),
        );
        let poll = fetched(&context.take_commands())
            .expect("polling resumed")
            .0;
        let mut context = Context::default();
        app.on_task(&mut context, poll, multi_select(4));
        let _ = context.take_commands();
        // Nothing is ticked, so there is nothing to send yet.
        let commands = act(&mut app, action_id(SEND));
        assert!(
            posted(&commands).is_none(),
            "a tick survived into the next question"
        );
    }

    #[test]
    fn a_permission_offering_always_allow_still_offers_deciding_once() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(
            &mut context,
            poll,
            TaskOutcome::Completed(
                br#"{"ask":{"id":9,"source":"claude","tool":"Edit","detail":"/tmp/README.md",
                "permission":true,"choices":[
                {"label":"Accept edits","description":"for the rest of this session"}]}}"#
                    .to_vec(),
            ),
        );
        let screen = painted(&context.take_commands()).expect("the question was painted");
        let lines = shown(&screen);
        for words in ["Accept edits", "Allow", "Deny", "Leave it for the terminal"] {
            assert!(
                lines.iter().any(|line| line.contains(words)),
                "no {words} on the panel: {lines:?}"
            );
        }
    }

    #[test]
    fn tapping_an_answer_sends_the_label_it_was_given() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, multiple_choice(7));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id("choice.1"));
        let (_, _, body) = posted(&commands).expect("the answer was sent");
        assert!(body.contains(r#""labels":["Every step"]"#), "{body}");
        assert!(body.contains(r#""id":7"#), "{body}");
        let confirmation = app.last.as_deref().unwrap();
        assert!(
            confirmation.starts_with("Answered Every step"),
            "{confirmation}"
        );
        assert!(!confirmation.contains("Left at the terminal"));
    }

    #[test]
    fn a_first_run_asks_for_the_address_and_touches_no_network() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: None,
            },
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_none(), "polled before pairing");
        let lines = shown(&painted(&commands).expect("a screen"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("kobo sidekick init")),
            "the screen never says what to run to get an address: {lines:?}"
        );
    }

    #[test]
    fn a_remembered_pairing_goes_straight_to_watching_and_polls_with_its_token() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:9331\nabc123".to_vec()),
            },
        );
        let commands = context.take_commands();
        let (_, url) = fetched(&commands).expect("watching starts a poll");
        assert_eq!(
            url,
            "https://192.168.1.5:9331/pending?token=abc123&all=true&wait=25"
        );
        assert_eq!(app.view, View::Watching);
    }

    #[test]
    fn a_pairing_written_with_a_trailing_newline_still_polls_with_the_right_token() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:9331\nabc123\n".to_vec()),
            },
        );
        let commands = context.take_commands();
        let (_, url) = fetched(&commands).expect("watching starts a poll");
        assert_eq!(
            url,
            "https://192.168.1.5:9331/pending?token=abc123&all=true&wait=25"
        );
    }

    #[test]
    fn a_pairing_with_nothing_after_the_newline_asks_to_be_paired_again() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:9331\n  \n".to_vec()),
            },
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_none(), "polled without a code");
        assert_eq!(app.view, View::Address);
    }

    #[test]
    fn typing_the_address_and_code_saves_the_pairing_and_starts_watching() {
        let mut app = Sidekick::default();
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
        // A bare host gets the daemon's port; the reader types one thing.
        app.keyboard = Keyboard::with_text("192.168.1.9");
        act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Code);
        app.keyboard = Keyboard::with_text("qk3mzp");
        let mut context = Context::default();
        app.on_action(&mut context, action_id("kb.enter"));
        let commands = context.take_commands();
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) => Some((key.clone(), value.clone())),
            _ => None,
        });
        assert_eq!(
            saved,
            Some((PAIRED.to_owned(), b"192.168.1.9:9331\nqk3mzp".to_vec()))
        );
        let (_, url) = fetched(&commands).expect("pairing starts the first poll");
        assert!(url.starts_with("https://192.168.1.9:9331/pending?token=qk3mzp"));
    }

    #[test]
    fn a_seventh_code_character_is_refused_at_the_keyboard() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: None,
            },
        );
        app.keyboard = Keyboard::with_text("192.168.1.9");
        act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Code);
        app.keyboard = Keyboard::with_text("qk3mzp");
        // The panel draws six boxes; a seventh character has no box to be
        // drawn in, so the key does nothing.
        act(&mut app, action_id("kb.r0c0"));
        assert_eq!(app.keyboard.text(), "qk3mzp");
    }

    #[test]
    fn an_address_that_could_not_be_one_is_refused_with_a_reason() {
        let mut app = Sidekick {
            view: View::Address,
            keyboard: Keyboard::with_text("what even is this?"),
            ..Sidekick::default()
        };
        let commands = act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Address, "a nonsense address moved on");
        let lines = shown(&painted(&commands).expect("a repaint with the reason"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("address shown on your computer")),
            "no reason shown: {lines:?}"
        );
    }

    #[test]
    fn a_question_off_the_wire_paints_the_command_and_three_answers() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo test --workspace"));
        let commands = context.take_commands();
        assert!(
            fetched(&commands).is_none(),
            "kept polling with a question on the panel"
        );
        let screen = painted(&commands).expect("the question was painted");
        let lines = shown(&screen);
        assert!(
            lines.iter().any(|line| line.contains("cargo test")),
            "the command is not on the panel: {lines:?}"
        );
        assert!(lines.iter().any(|line| line.contains("Codex")), "{lines:?}");
        for label in ["Allow", "Deny", "Leave it for the terminal"] {
            assert!(
                lines.iter().any(|line| line.contains(label)),
                "no {label} on the panel: {lines:?}"
            );
        }
    }

    #[test]
    fn an_empty_poll_asks_again_without_repainting() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, TaskOutcome::Completed(b"{}".to_vec()));
        let commands = context.take_commands();
        // A breath first. The daemon holds a poll open when nothing is
        // waiting, so an empty answer means it answered early, and asking
        // again the same instant is a loop rather than a watch.
        let nap = slept(&commands).expect("the loop stopped");
        assert!(
            painted(&commands).is_none(),
            "an empty poll repainted an unchanged screen"
        );
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        assert!(
            fetched(&context.take_commands()).is_some(),
            "the loop did not start again after the nap"
        );
    }

    #[test]
    fn allowing_posts_the_tap_and_watching_resumes_naming_the_outcome() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo test"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(ALLOW));
        let (task, url, body) = posted(&commands).expect("the answer was sent");
        assert_eq!(url, "https://192.168.1.5:9331/answer");
        assert!(body.contains(r#""id":4"#), "{body}");
        assert!(body.contains(r#""choice":"allow""#), "{body}");
        assert!(body.contains(r#""token":"abc123""#), "{body}");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":true}"#.to_vec()),
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "watching never resumed");
        let lines = shown(&painted(&commands).expect("back to watching"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Allowed") && line.contains("cargo test")),
            "the outcome is not stated: {lines:?}"
        );
    }

    #[test]
    fn back_on_a_question_leaves_it_for_the_terminal() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(7, "rm -rf ./build"));
        let _ = context.take_commands();
        let commands = act(&mut app, ActionId::BACK);
        let (_, _, body) = posted(&commands).expect("dismissal still answers");
        assert!(body.contains(r#""choice":"pass""#), "{body}");
    }

    #[test]
    fn a_dead_daemon_is_named_once_and_polling_resumes_after_a_nap() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(
            &mut context,
            poll,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        let commands = context.take_commands();
        let advice = kobo_sdk::Failure::of(kobo_sdk::TaskError::Unreachable).advice;
        let lines = shown(&painted(&commands).expect("the trouble was stated"));
        assert!(
            lines.iter().any(|line| line.contains(advice)),
            "no advice on the panel: {lines:?}"
        );
        let nap = slept(&commands).expect("no retry was scheduled");
        assert!(fetched(&commands).is_none(), "retried without the nap");
        let mut context = Context::default();
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "the nap never woke the loop");
        // The poll that then succeeds takes the banner down with one repaint.
        let poll = fetched(&commands).expect("polling again").0;
        let mut context = Context::default();
        app.on_task(&mut context, poll, TaskOutcome::Completed(b"{}".to_vec()));
        let commands = context.take_commands();
        let lines = shown(&painted(&commands).expect("the recovery repaints once"));
        assert!(
            !lines.iter().any(|line| line.contains(advice)),
            "the trouble outlived it: {lines:?}"
        );
    }

    #[test]
    fn a_failed_answer_puts_the_question_back_with_the_reason() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo build"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(DENY));
        let (task, _, _) = posted(&commands).expect("the answer was sent");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        let lines = shown(&painted(&context.take_commands()).expect("a repaint"));
        assert!(
            lines.iter().any(|line| line.contains("cargo build")),
            "the question left the panel with nobody told: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("Deny")),
            "the answers left with it: {lines:?}"
        );
    }

    #[test]
    fn changing_the_pairing_starts_from_the_current_address() {
        let (mut app, _) = paired();
        let commands = act(&mut app, action_id(REPAIR));
        assert_eq!(app.view, View::Address);
        let lines = shown(&painted(&commands).expect("the pairing screen"));
        assert!(
            lines.iter().any(|line| line.contains("192.168.1.5:9331")),
            "the address must be edited from scratch: {lines:?}"
        );
    }

    #[test]
    fn a_command_too_long_for_the_panel_is_cut_and_the_cut_is_marked() {
        let long = "x".repeat(2000);
        assert_eq!(super::trimmed(&long).chars().count(), super::MAX_DETAIL + 1);
        assert!(super::trimmed(&long).ends_with('…'));
        assert_eq!(super::trimmed("cargo test"), "cargo test");
    }

    #[test]
    fn an_answer_the_daemon_rejects_is_not_reported_as_decided() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo publish"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(ALLOW));
        let (task, _, _) = posted(&commands).expect("the answer was sent");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":false}"#.to_vec()),
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "watching never resumed");
        let lines = shown(&painted(&commands).expect("a repaint"));
        assert!(
            !lines.iter().any(|line| line.contains("Allowed")),
            "claimed a decision that never landed: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("gone before")),
            "the miss is not stated: {lines:?}"
        );
    }

    #[test]
    fn a_question_arriving_mid_repair_does_not_take_the_typing_screen() {
        let (mut app, poll) = paired();
        let _ = act(&mut app, action_id(REPAIR));
        assert_eq!(app.view, View::Address);
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(6, "cargo run"));
        let commands = context.take_commands();
        assert_eq!(app.view, View::Address, "a question took the typing screen");
        assert!(fetched(&commands).is_none(), "polled while re-pairing");
    }

    #[test]
    fn a_poll_from_the_old_pairing_cannot_answer_into_the_new() {
        let (mut app, old_poll) = paired();
        let _ = act(&mut app, action_id(REPAIR));
        app.keyboard = Keyboard::with_text("192.168.1.77");
        let _ = act(&mut app, action_id("kb.enter"));
        app.keyboard = Keyboard::with_text("zzzzzz");
        let mut context = Context::default();
        // Real task numbers never repeat; a fresh test context restarts
        // them, so spend a few keeping the new poll's id off the old one's.
        for _ in 0..3 {
            let _ = context.spawn(Task::Sleep { seconds: 1 });
        }
        app.on_action(&mut context, action_id("kb.enter"));
        let commands = context.take_commands();
        let (new_poll, url) = fetched(&commands).expect("the new pairing polls");
        assert_ne!(new_poll, old_poll, "the old poll still speaks");
        assert!(url.contains("192.168.1.77"), "{url}");
        assert!(url.contains("token=zzzzzz"), "{url}");
        // The old poll comes back bearing a question: it lands on nothing.
        let mut context = Context::default();
        app.on_task(&mut context, old_poll, question(4, "rm -rf /"));
        let commands = context.take_commands();
        assert_eq!(app.view, View::Watching, "a stale poll took the panel");
        assert!(app.ask.is_none(), "a stale question was kept");
        assert!(fetched(&commands).is_none(), "a stale poll spun the loop");
    }

    #[test]
    fn ignoring_a_question_says_so_without_claiming_a_decision() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(9, "npx create-react-app"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(IGNORE));
        let (task, _, body) = posted(&commands).expect("ignoring still answers");
        assert!(body.contains(r#""choice":"pass""#), "{body}");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":true}"#.to_vec()),
        );
        let lines = shown(&painted(&context.take_commands()).expect("watching again"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Left at the terminal")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_question_says_which_terminal_on_which_computer_is_asking() {
        // With one agent the tool was enough. With three of them on two
        // machines, "shell asks" is not a question anybody can answer.
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, one_of_the_fleet());
        let lines = shown(&painted(&context.take_commands()).expect("a screen"));
        let byline = lines
            .iter()
            .find(|line| line.contains("Bash"))
            .unwrap_or_else(|| panic!("no byline in {lines:?}"));
        assert!(byline.contains("cobalt ab12"), "{byline}");
        assert!(byline.contains("192.168.1.5:9331"), "{byline}");
    }

    #[test]
    fn two_terminals_asking_at_once_are_both_on_the_board_with_their_own_answers() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, fleet());
        assert_eq!(app.view, View::Board);
        let lines = shown(&painted(&context.take_commands()).expect("a screen"));
        let said = lines.join(" | ");
        assert!(said.contains("cobalt ab12"), "{said}");
        assert!(said.contains("notes cd34"), "{said}");
        assert!(said.contains("cargo test --workspace"), "{said}");
        assert!(said.contains("git push --force-with-lease"), "{said}");

        // Opening one of them answers that one and nothing else: the other
        // terminal is still waiting on the daemon.
        let commands = act(&mut app, action_id(&super::board_action(1)));
        assert_eq!(app.view, View::Asking);
        assert_eq!(app.ask.as_ref().map(|ask| ask.id), Some(2));
        let drawn = shown(&painted(&commands).expect("a screen")).join(" | ");
        assert!(drawn.contains("git push --force-with-lease"), "{drawn}");
        assert!(!drawn.contains("cargo test --workspace"), "{drawn}");
    }

    #[test]
    fn a_fleet_snapshot_preserves_session_identity_for_each_board_row() {
        let snapshot = r#"{"version":"4","asks":[
                {"id":1,"source":"claude","session":"cobalt · ab12","tool":"Bash","detail":"cargo test"},
                {"id":2,"source":"codex","session":"cobalt · cd34","tool":"shell","detail":"git status"}
            ]}"#;
        let asks = super::read_asks(snapshot.as_bytes());
        assert_eq!(asks.len(), 2);
        assert_eq!(super::session_suffix(&asks[0]), " · cobalt · ab12");
        assert_eq!(asks[1].source, "codex");
    }

    #[test]
    fn incomplete_pairing_stays_editable_without_network_or_save() {
        let mut app = Sidekick {
            view: View::Code,
            address: "example:9331".into(),
            keyboard: Keyboard::with_text("abc"),
            ..Sidekick::default()
        };
        let commands = act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Code);
        assert_eq!(app.keyboard.text(), "abc");
        assert!(fetched(&commands).is_none());
        assert!(!commands
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
        assert!(shown(&painted(&commands).expect("validation feedback"))
            .join(" ")
            .contains("six characters"));
        let _ = act(&mut app, ActionId::BACK);
        assert_eq!(app.view, View::Address);
        assert!(app.trouble.is_none());
    }

    #[test]
    fn long_requests_and_choice_pages_preserve_text_and_reachable_actions() {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Sidekick::default(), metrics).context();
            let (mut app, _) = paired();
            app.view = View::Asking;
            app.ask=Some(super::Ask {
                id:7,source:"codex".into(),session:"project ab12".into(),tool:"shell".into(),
                detail:"printf 'first line  second line' && cargo test --workspace --all-features --locked; ".repeat(7),
                choices:Vec::new(), permission:true,multi:false,
            });
            for multiple in [false, true] {
                if multiple {
                    let ask = app.ask.as_mut().unwrap();
                    ask.permission = false;
                    ask.multi = true;
                    ask.choices = (0..7)
                        .map(|index| super::Choice {
                            label: format!("Section {index}"),
                            description:
                                "Include this section and retain all of its existing information."
                                    .into(),
                        })
                        .collect();
                    app.ticked = vec![false; 7];
                }
                let ask = app.ask.as_ref().unwrap();
                let pages = app.question_pages(&context, ask);
                assert_eq!(
                    pages.iter().map(|p| p.detail.as_str()).collect::<String>(),
                    super::trimmed(&ask.detail)
                );
                assert_eq!(
                    pages
                        .iter()
                        .flat_map(|p| p.choices.iter().copied())
                        .collect::<Vec<_>>(),
                    (0..ask.choices.len()).collect::<Vec<_>>()
                );
                let mut decision_rect: Option<kobo_ui::Rect> = None;
                for (page, content) in pages.iter().enumerate() {
                    let screen = app
                        .question_screen(ask, content, page, pages.len())
                        .with_own_back(true);
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        !diagnostics.has_errors(),
                        "{scale:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                    let rect = diagnostics
                        .layout
                        .rect_of_action(action_id(IGNORE))
                        .expect("leave remains present");
                    assert_eq!(
                        diagnostics
                            .layout
                            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action_id(IGNORE))
                    );
                    if let Some(previous) = decision_rect {
                        assert_eq!(
                            (rect.x, rect.width, rect.height),
                            (previous.x, previous.width, previous.height)
                        );
                        assert!(
                            (rect.y - previous.y).abs() <= 1,
                            "only pixel rounding may move the leave control"
                        );
                    }
                    decision_rect = Some(rect);
                    if !multiple {
                        for action in [ALLOW, DENY] {
                            let rect = diagnostics
                                .layout
                                .rect_of_action(action_id(action))
                                .expect("decision visible");
                            assert_eq!(
                                diagnostics
                                    .layout
                                    .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                                Some(action_id(action))
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_waiting_terminal_is_reachable_at_every_text_size() {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let context = kobo_sdk::AppRunner::with_metrics(Sidekick::default(), metrics).context();
            let (mut app, _) = paired();
            app.view = View::Board;
            app.board = (0..20)
                .map(|id| super::Ask {
                    id,
                    source: "codex".into(),
                    session: format!("terminal {id}"),
                    tool: "shell".into(),
                    detail: "cargo test --workspace --locked".into(),
                    choices: Vec::new(),
                    permission: true,
                    multi: false,
                })
                .collect();
            let pages = app.board_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..20).collect::<Vec<_>>()
            );
            for (page, indices) in pages.iter().enumerate() {
                app.page = page;
                let screen = app.board(&context);
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    !diagnostics.has_errors(),
                    "{scale:?}: {:?}",
                    diagnostics.issues
                );
                for &index in indices {
                    let action = action_id(&super::board_action(index));
                    let rect = diagnostics
                        .layout
                        .rect_of_action(action)
                        .expect("visible question");
                    assert_eq!(
                        diagnostics
                            .layout
                            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action)
                    );
                }
            }
        }
    }

    #[test]
    fn turning_choice_pages_keeps_selections_and_never_posts_a_decision() {
        let (mut app, poll) = paired();
        app.on_task(&mut Context::default(), poll, multi_select(9));
        let _ = act(&mut app, action_id("choice.0"));
        for action in [super::NEXT, super::NEXT, super::PREVIOUS, super::PREVIOUS] {
            assert!(posted(&act(&mut app, action_id(action))).is_none());
            assert!(app.is_ticked(0));
        }
        assert_eq!(app.page, 0);
        let (_, _, body) = posted(&act(&mut app, action_id(SEND))).expect("send selected answers");
        assert!(body.contains("Introduction"));
    }
    #[test]
    fn off_page_answers_are_kept_and_double_submit_posts_only_once() {
        let (mut app, poll) = paired();
        app.on_task(&mut Context::default(), poll, multi_select(9));
        app.ask.as_mut().unwrap().choices = (0..7)
            .map(|index| super::Choice {
                label: format!("Choice {index}"),
                description: "Include this section and keep the existing content.".into(),
            })
            .collect();
        app.ticked = vec![false; 7];
        let context = kobo_sdk::AppRunner::new(Sidekick::default()).context();
        let pages = app.question_pages(&context, app.ask.as_ref().unwrap());
        assert!(pages.len() > 1, "fixture must actually have another page");
        let first = pages[0].choices[0];
        let last = *pages.last().unwrap().choices.last().unwrap();
        assert_ne!(first, last);
        assert!(posted(&act(&mut app, action_id(&super::chosen_action(first)))).is_none());
        for _ in 1..pages.len() {
            assert!(posted(&act(&mut app, action_id(super::NEXT))).is_none());
        }
        assert_eq!(app.page, pages.len() - 1);
        let screen = app.screen(&context);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let action = action_id(&super::chosen_action(last));
        let rect = layout
            .rect_of_action(action)
            .expect("last-page answer is visible");
        assert_eq!(
            layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
            Some(action)
        );
        assert!(posted(&act(&mut app, action)).is_none());
        assert!(app.is_ticked(first));
        assert!(app.is_ticked(last));
        let (_, _, body) = posted(&act(&mut app, action_id(SEND))).expect("one submission");
        assert!(body.contains(&format!("Choice {first}")));
        assert!(body.contains(&format!("Choice {last}")));
        assert_eq!(app.view, View::Sending);
        for stale in [SEND, ALLOW, DENY, IGNORE, super::NEXT] {
            assert!(
                posted(&act(&mut app, action_id(stale))).is_none(),
                "a repeated tap must not post again"
            );
        }
    }

    #[test]
    fn incomplete_pairing_feedback_fits_every_text_size() {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let mut runner = kobo_sdk::AppRunner::with_metrics(
                Sidekick {
                    view: View::Code,
                    keyboard: Keyboard::with_text("abc"),
                    ..Sidekick::default()
                },
                metrics,
            );
            let screen = painted(&runner.action(action_id("kb.enter"))).expect("validation screen");
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                !diagnostics.has_errors(),
                "{scale:?}: {:?}",
                diagnostics.issues
            );
        }
    }

    fn oversized_choice(label: String, description: String) -> Sidekick {
        let (mut app, _) = paired();
        app.view = View::Asking;
        app.poll = None;
        app.ask = Some(super::Ask {
            id: 42,
            source: "codex".into(),
            session: "project ab12".into(),
            tool: "shell".into(),
            detail: "Choose the sections to retain.".into(),
            choices: vec![
                super::Choice { label, description },
                super::Choice {
                    label: "Keep the deployment notes".into(),
                    description: "Retain the notes too.".into(),
                },
            ],
            permission: false,
            multi: true,
        });
        app.ticked = vec![false; 2];
        app
    }

    fn migration_description() -> String {
        "Keep all existing migrations, validate their checksums, preserve the deployment order, and report every validation error before applying changes. ".repeat(5)
    }

    fn assert_reachable(screen: &Screen, metrics: &DisplayMetrics, action: &str) {
        let diagnostics = screen.diagnostics(metrics, &Chrome::measuring(true));
        assert!(
            !diagnostics.has_errors(),
            "{:?}: {:?}",
            metrics.text_scale,
            diagnostics.issues
        );
        let action = action_id(action);
        let rect = diagnostics
            .layout
            .rect_of_action(action)
            .expect("visible action");
        assert_eq!(
            diagnostics
                .layout
                .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
            Some(action)
        );
    }

    #[test]
    fn oversized_choice_text_is_complete_and_every_part_fits_the_wire_and_panel() {
        let cases = [
            (
                "Preserve the migration plan".into(),
                migration_description(),
            ),
            (
                "Preserve the migration and rollback checks. ".repeat(30),
                migration_description(),
            ),
            ("café_à_revoir_".repeat(90), String::new()),
        ];
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for (label, description) in &cases {
                for permission in [false, true] {
                    let mut app = oversized_choice(label.clone(), description.clone());
                    app.ask.as_mut().unwrap().permission = permission;
                    app.ticked[0] = true;
                    let runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
                    let app = runner.app();
                    let ask = app.ask.as_ref().unwrap();
                    let pages = app.question_pages(&runner.context(), ask);
                    let mut recovered = String::new();
                    for (page, content) in pages.iter().enumerate() {
                        if content.choices.contains(&0) {
                            recovered.push_str(content.choice_text.as_deref().unwrap_or(label));
                            if content.choice_text.is_none() && !description.is_empty() {
                                recovered.push_str("\n\n");
                                recovered.push_str(description);
                            }
                        }
                        // Context::set_screen performs wire and glyph validation,
                        // which layout diagnostics alone cannot exercise.
                        let mut context = runner.context();
                        context.set_screen(
                            app.question_screen(ask, content, page, pages.len())
                                .with_own_back(true),
                        );
                        let screen = painted(context.commands()).expect("wire-valid screen");
                        assert_reachable(&screen, &metrics, IGNORE);
                        assert_reachable(&screen, &metrics, SEND);
                        for &index in &content.choices {
                            assert_reachable(&screen, &metrics, &super::chosen_action(index));
                        }
                        for action in [ALLOW, DENY] {
                            if permission {
                                assert_reachable(&screen, &metrics, action);
                            } else {
                                assert!(screen
                                    .layout_for(&metrics)
                                    .rect_of_action(action_id(action))
                                    .is_none());
                            }
                        }
                    }
                    let expected = if description.is_empty() {
                        label.clone()
                    } else {
                        format!("{label}\n\n{description}")
                    };
                    assert_eq!(
                        recovered, expected,
                        "all name/description bytes survive paging"
                    );
                }
            }
        }
    }

    #[test]
    fn oversized_choice_callbacks_retain_answers_bounds_and_retry_without_double_submit() {
        // Layout and wire validation above cover all nine scales. Exercise the
        // full asynchronous flow at both ends of the supported size range.
        for scale in [TextScale::Default, TextScale::Largest] {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let label = "Preserve every migration, including rollback checks. ".repeat(20);
            let mut runner = kobo_sdk::AppRunner::with_metrics(
                oversized_choice(label.clone(), migration_description()),
                metrics,
            );
            let pages = runner
                .app()
                .question_pages(&runner.context(), runner.app().ask.as_ref().unwrap());
            assert!(pages.len() > 2);
            runner.action(action_id(super::PREVIOUS));
            assert_eq!(runner.app().page, 0);
            let first = pages
                .iter()
                .position(|page| page.choices.contains(&0))
                .unwrap();
            for _ in 0..first {
                runner.action(action_id(super::NEXT));
            }
            assert!(posted(&runner.action(action_id("choice.0"))).is_none());
            for _ in 0..pages.len() + 2 {
                let commands = runner.action(action_id(super::NEXT));
                assert!(posted(&commands).is_none());
                if let Some(screen) = painted(&commands) {
                    assert_reachable(&screen, &metrics, IGNORE);
                    assert_reachable(&screen, &metrics, SEND);
                }
                assert!(runner.app().is_ticked(0));
            }
            assert_eq!(runner.app().page, pages.len() - 1);
            runner.action(action_id("choice.1"));
            for _ in 0..pages.len() + 2 {
                runner.action(action_id(super::PREVIOUS));
            }
            assert_eq!(runner.app().page, 0);
            assert_eq!(runner.app().ticked, vec![true, true]);
            let (task, _, body) = posted(&runner.action(action_id(SEND))).expect("one post");
            let body = kobo_json::parse(&body).unwrap();
            let labels = body.get("labels").unwrap().as_array().unwrap();
            assert_eq!(labels[0].as_str(), Some(label.as_str()));
            assert_eq!(labels[1].as_str(), Some("Keep the deployment notes"));
            for action in [SEND, ALLOW, DENY, IGNORE, super::NEXT, "choice.0"] {
                assert!(posted(&runner.action(action_id(action))).is_none());
            }
            assert!(posted(&runner.action(ActionId::BACK)).is_none());
            let commands =
                runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Unauthorized));
            assert_eq!(runner.app().view, View::Asking);
            assert_eq!(runner.app().ticked, vec![true, true]);
            assert_reachable(&painted(&commands).expect("retry screen"), &metrics, SEND);
            let retry_pages = runner
                .app()
                .question_pages(&runner.context(), runner.app().ask.as_ref().unwrap())
                .len();
            for _ in 0..retry_pages + 2 {
                if let Some(screen) = painted(&runner.action(action_id(super::NEXT))) {
                    assert_reachable(&screen, &metrics, SEND);
                }
            }
            assert_eq!(runner.app().page, retry_pages - 1);
            let (task, _, _) = posted(&runner.action(action_id(SEND))).expect("retry post");
            let commands =
                runner.task_outcome(task, TaskOutcome::Completed(br#"{"ok":true}"#.to_vec()));
            let (poll, _) = fetched(&commands).expect("poll resumes");
            runner.task_outcome(poll, multi_select(43));
            assert_eq!(runner.app().page, 0);
            assert_eq!(runner.app().ticked, vec![false; 3]);
        }
    }

    #[test]
    fn a_single_choice_continuation_submits_its_exact_original_label() {
        let metrics = DisplayMetrics {
            text_scale: TextScale::Largest,
            ..CLARA_BW_METRICS
        };
        let label = "Keep the migration plan exactly as approved. ".repeat(25);
        let mut app = oversized_choice(label.clone(), migration_description());
        app.ask.as_mut().unwrap().multi = false;
        app.ask.as_mut().unwrap().detail.clear();
        let mut runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
        let pages = runner
            .app()
            .question_pages(&runner.context(), runner.app().ask.as_ref().unwrap());
        assert!(pages[0].choice_text.is_some(), "no empty introductory page");
        let screen = painted(&runner.action(action_id(super::NEXT))).expect("continuation");
        assert_reachable(&screen, &metrics, "choice.0");
        assert!(screen
            .layout_for(&metrics)
            .rect_of_action(action_id(SEND))
            .is_none());
        let (_, _, body) = posted(&runner.action(action_id("choice.0"))).expect("single answer");
        let body = kobo_json::parse(&body).unwrap();
        assert_eq!(
            body.get("labels").unwrap().as_array().unwrap()[0].as_str(),
            Some(label.as_str())
        );
        assert!(posted(&runner.action(action_id("choice.0"))).is_none());
    }
}
