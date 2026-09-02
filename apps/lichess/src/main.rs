//! An unofficial Lichess Board API client for Cobalt.
//!
//! The application names the `lichess` secret but never receives its value.
//! Live HTTP streams, redirects, cancellation, and credential attachment stay
//! in the runtime.

mod api;
mod chess;
mod model;

use api::{BoardRecord, Event};
use kobo_json::{ObjectBuilder, Value};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, ControlState, Failure, Glyph, Heartbeat, KoboApp,
    Screen, ScreenBuilder, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use model::{
    Account, ApplyState, Challenge, ChallengeDirection, Color, Game, GameSummary, Session,
};
#[cfg(any(test, debug_assertions))]
use model::{ChallengeTime, FullGame, Player, ServerState};
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::time::Duration;

const SESSION_KEY: &str = "lichess.session.v1";
const PUZZLE_KEY: &str = "lichess.puzzles.v1";
const MAX_STORED_PUZZLES: usize = 32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Home,
    Puzzles,
    Solve,
    PuzzleResult,
    Play,
    Pairing,
    Challenge,
    Game,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum AccountState {
    #[default]
    Unknown,
    Checking,
    Ready(Account),
    Missing,
    Invalid,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Pending {
    Account,
    Playing,
    Puzzle,
    EventOpen,
    EventNext,
    EventRetry,
    EventClose,
    Seek,
    SeekGrace,
    BoardOpen(String),
    BoardNext(String),
    BoardRetry(String),
    BoardClose(String),
    Action(GameAction),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GameAction {
    Move(String),
    Resign,
    Abort,
    OfferDraw,
    AcceptDraw,
    DeclineDraw,
    ClaimVictory,
    AcceptChallenge(String),
    DeclineChallenge(String),
}

impl GameAction {
    fn label(&self) -> &'static str {
        match self {
            Self::Move(_) => "Submitting move",
            Self::Resign => "Resigning",
            Self::Abort => "Aborting game",
            Self::OfferDraw => "Offering draw",
            Self::AcceptDraw => "Accepting draw",
            Self::DeclineDraw => "Declining draw",
            Self::ClaimVictory => "Claiming victory",
            Self::AcceptChallenge(_) => "Accepting challenge",
            Self::DeclineChallenge(_) => "Declining challenge",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingMove {
    movement: String,
    at_ply: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Promotion {
    from: String,
    to: String,
    choices: Vec<char>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Confirmation {
    Resign,
    Abort,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Puzzle {
    id: String,
    fen: String,
    solution: Vec<String>,
    cursor: usize,
}

impl Puzzle {
    fn read(value: &Value) -> Option<Self> {
        let puzzle = value.get("puzzle")?;
        let id = puzzle.get("id")?.as_str()?.to_owned();
        let solution = puzzle
            .get("solution")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .filter(|movement| valid_move(movement))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let game = value.get("game")?;
        let initial_ply = usize::try_from(puzzle.get("initialPly")?.as_i64()?).ok()?;
        let fen = game
            .get("fen")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| chess::puzzle_position(game.get("pgn")?.as_str()?, initial_ply))?;
        (!solution.is_empty()).then_some(Self {
            id,
            fen,
            solution,
            cursor: 0,
        })
    }

    fn stored(&self) -> Value {
        ObjectBuilder::new()
            .set("id", self.id.clone())
            .set("fen", self.fen.clone())
            .set("solution", self.solution.clone())
            .set("cursor", u32::try_from(self.cursor).unwrap_or(u32::MAX))
            .build()
    }

    fn from_stored(value: &Value) -> Option<Self> {
        let solution = value
            .get("solution")?
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .filter(|movement| valid_move(movement))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let cursor = usize::try_from(value.get("cursor")?.as_i64()?).ok()?;
        let fen = value.get("fen")?.as_str()?.to_owned();
        chess::replay(&fen, &[])?;
        (!solution.is_empty() && cursor <= solution.len()).then_some(Self {
            id: value.get("id")?.as_str()?.to_owned(),
            fen,
            solution,
            cursor,
        })
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "load completion, stream ownership, menu state, and suspension are independent lifecycle facts"
)]
struct Lichess {
    route: Route,
    account: AccountState,
    loaded_session: bool,
    loaded_puzzles: bool,
    playing_ready: bool,
    session: Option<Session>,
    summaries: Vec<GameSummary>,
    game: Option<Game>,
    challenge: Option<Challenge>,
    tasks: BTreeMap<TaskId, Pending>,
    event_open: bool,
    board_open: Option<String>,
    seek_task: Option<TaskId>,
    seek_waiting: bool,
    seek_baseline: BTreeSet<String>,
    pending_action: Option<GameAction>,
    pending_move: Option<PendingMove>,
    selected: Option<String>,
    promotion: Option<Promotion>,
    confirmation: Option<Confirmation>,
    menu_open: bool,
    notice: Option<String>,
    clock: Heartbeat,
    total_ticks: u64,
    gone_tick: u64,
    event_backoff: u32,
    board_backoff: u32,
    suspended: bool,
    puzzles: Vec<Puzzle>,
    current_puzzle: usize,
    puzzle_wrong: u8,
}

impl Default for Lichess {
    fn default() -> Self {
        Self {
            route: Route::Home,
            account: AccountState::Unknown,
            loaded_session: false,
            loaded_puzzles: false,
            playing_ready: false,
            session: None,
            summaries: Vec::new(),
            game: None,
            challenge: None,
            tasks: BTreeMap::new(),
            event_open: false,
            board_open: None,
            seek_task: None,
            seek_waiting: false,
            seek_baseline: BTreeSet::new(),
            pending_action: None,
            pending_move: None,
            selected: None,
            promotion: None,
            confirmation: None,
            menu_open: false,
            notice: None,
            clock: Heartbeat::every(1),
            total_ticks: 0,
            gone_tick: 0,
            event_backoff: 1,
            board_backoff: 1,
            suspended: false,
            puzzles: Vec::new(),
            current_puzzle: 0,
            puzzle_wrong: 0,
        }
    }
}

impl Lichess {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Home));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Home => self.home(),
            Route::Puzzles => self.puzzles_screen(),
            Route::Solve => self.solve_screen(),
            Route::PuzzleResult => self.puzzle_result(),
            Route::Play => self.play_screen(),
            Route::Pairing => self.pairing_screen(),
            Route::Challenge => self.challenge_screen(),
            Route::Game => self.game_screen(),
        }
    }

    fn home(&self) -> Screen {
        let games =
            self.summaries.len()
                + usize::from(self.game.as_ref().is_some_and(|game| {
                    !self.summaries.iter().any(|summary| summary.id == game.id)
                }));
        let mut screen = ScreenBuilder::new("lichess-home")
            .top_bar("Lichess")
            .tiles([
                (
                    "puzzles",
                    format!("Puzzles\n{} ready", self.remaining_puzzles()),
                    Glyph::Grid,
                ),
                (
                    "play",
                    format!("Play\n{games} active"),
                    Glyph::Play,
                ),
            ])
            .section("Board API")
            .text("Quick pairing is rated 10+0 Rapid. Live games require Cobalt 0.3.4, protocol 11, and the named secret lichess.");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen.build()
    }

    fn puzzles_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-puzzles")
            .top_bar("Puzzles")
            .facts([
                ("Ready", self.remaining_puzzles().to_string()),
                (
                    "Current",
                    self.puzzles
                        .get(self.current_puzzle)
                        .map_or_else(|| "—".to_owned(), |puzzle| puzzle.id.clone()),
                ),
            ]);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let fetching = self.has_pending(|pending| matches!(pending, Pending::Puzzle));
        screen
            .button_with_state(
                "new-puzzles",
                if fetching {
                    "Downloading…"
                } else {
                    "Download 32 puzzles"
                },
                if fetching {
                    ControlState::Disabled
                } else {
                    ControlState::Enabled
                },
            )
            .button_with_state(
                "solve",
                "Resume puzzle",
                if self.remaining_puzzles() > 0 {
                    ControlState::Enabled
                } else {
                    ControlState::Disabled
                },
            )
            .secondary("Puzzle solving is local and does not change your Lichess rating.")
            .build()
    }

    fn solve_screen(&self) -> Screen {
        let Some(puzzle) = self.puzzles.get(self.current_puzzle) else {
            return ScreenBuilder::new("lichess-solve")
                .top_bar("Solve")
                .empty_state("No puzzle is ready. Download a session on Wi-Fi.")
                .build();
        };
        let orientation = if chess::side_to_move(&puzzle.fen) == Some('b') {
            Color::Black
        } else {
            Color::White
        };
        let mut screen = ScreenBuilder::new("lichess-solve")
            .top_bar("Solve")
            .secondary(format!("{} to move", orientation.name()));
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .board(
                8,
                board_cells(
                    &puzzle.fen,
                    orientation,
                    self.selected.as_deref(),
                    None,
                    None,
                ),
            )
            .secondary("Tap a piece, then its destination.")
            .button("skip-puzzle", "Skip puzzle")
            .build()
    }

    fn puzzle_result(&self) -> Screen {
        ScreenBuilder::new("lichess-puzzle-result")
            .top_bar("Puzzle result")
            .heading(if self.puzzle_wrong > 1 {
                "Not solved"
            } else {
                "Solved"
            })
            .text(if self.puzzle_wrong > 1 {
                self.puzzles
                    .get(self.current_puzzle)
                    .and_then(|puzzle| puzzle.solution.first())
                    .map_or_else(
                        || "The solution could not be read.".to_owned(),
                        |movement| format!("The first move was {movement}."),
                    )
            } else {
                "Counted only on this reader.".to_owned()
            })
            .primary_button("next-puzzle", "Next puzzle")
            .build()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one declarative screen keeps all account, challenge, and ongoing-game states visible"
    )]
    fn play_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-play").top_bar("Play");
        screen = match &self.account {
            AccountState::Unknown => screen.text("Open Play to validate the named Lichess secret."),
            AccountState::Checking => screen.activity("Validating Lichess account", None),
            AccountState::Ready(account) => {
                screen.secondary(format!("Signed in as {}", account.username))
            }
            AccountState::Missing => screen
                .banner(
                    BannerLevel::Attention,
                    "No secret named lichess is installed.",
                )
                .text("Install it with kobo secret set lichess. The token is never shown on this screen."),
            AccountState::Invalid => screen
                .banner(
                    BannerLevel::Attention,
                    "The Lichess token is invalid, expired, or lacks Board API access.",
                )
                .text("Replace the named secret lichess with a token granting board:play."),
            AccountState::Failed(message) => {
                screen.banner(BannerLevel::Attention, message)
            }
        };
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if let Some(challenge) = &self.challenge {
            screen = screen.section("Incoming challenge").rows([(
                "open-challenge",
                challenge.challenger.clone(),
                challenge.description(),
                Glyph::Play,
            )]);
        }
        if !self.summaries.is_empty() {
            screen = screen
                .section("Ongoing games")
                .rows(self.summaries.iter().enumerate().map(|(index, game)| {
                    (
                        format!("resume-{index}"),
                        game.opponent.clone(),
                        format!(
                            "{} · {}",
                            if game.is_my_turn {
                                "your move"
                            } else {
                                "their move"
                            },
                            game.last_move.as_deref().unwrap_or("no moves")
                        ),
                        Glyph::Grid,
                    )
                }));
        }
        if let Some(game) = &self.game {
            screen = screen.section("Current board").rows([(
                "resume-current",
                game.opponent().display(),
                if game.my_turn() {
                    "your move"
                } else {
                    "their move"
                },
                Glyph::Grid,
            )]);
        }
        let ready = matches!(self.account, AccountState::Ready(_))
            && self.event_open
            && self.playing_ready
            && self.seek_task.is_none()
            && !self.seek_waiting
            && self.game.as_ref().is_none_or(|game| !game.active())
            && self.pending_action.is_none();
        screen
            .button_with_state(
                "quick-pair",
                "Quick pair · Rated 10+0",
                if ready {
                    ControlState::Enabled
                } else {
                    ControlState::Disabled
                },
            )
            .button_with_state(
                "refresh-play",
                "Refresh account and games",
                if self
                    .has_pending(|pending| matches!(pending, Pending::Account | Pending::Playing))
                {
                    ControlState::Disabled
                } else {
                    ControlState::Enabled
                },
            )
            .secondary(
                if matches!(self.account, AccountState::Ready(_)) && !self.event_open {
                    "Preparing the event stream before pairing."
                } else {
                    "Closing a pending seek cancels it. It is never replayed automatically."
                },
            )
            .build()
    }

    fn pairing_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-pairing")
            .top_bar("Quick pairing")
            .heading("Rated 10+0 Rapid")
            .activity("Waiting for Lichess", None)
            .secondary(self.clock.waited_words())
            .text("The account event stream was opened before this seek. Keep this screen open; Cancel closes the one pending seek.");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .button_with_state(
                "cancel-seek",
                "Cancel pairing",
                enabled(self.seek_task.is_some() || self.seek_waiting),
            )
            .build()
    }

    fn challenge_screen(&self) -> Screen {
        let Some(challenge) = &self.challenge else {
            return ScreenBuilder::new("lichess-challenge")
                .top_bar("Challenge")
                .empty_state("That challenge is no longer active.")
                .build();
        };
        let mut screen = ScreenBuilder::new("lichess-challenge")
            .top_bar("Incoming challenge")
            .heading(challenge.challenger.clone())
            .text(challenge.description());
        if !challenge.supported() {
            screen = screen.banner(
                BannerLevel::Attention,
                "This client accepts standard clock games only.",
            );
        }
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let pending = self.pending_action.is_some();
        screen
            .button_with_state(
                "accept-challenge",
                "Accept",
                if challenge.supported() && !pending {
                    ControlState::Enabled
                } else {
                    ControlState::Disabled
                },
            )
            .button_with_state(
                "decline-challenge",
                "Decline",
                if pending {
                    ControlState::Disabled
                } else {
                    ControlState::Enabled
                },
            )
            .build()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the board, clocks, result, promotion, confirmation, and pending overlays form one screen"
    )]
    fn game_screen(&self) -> Screen {
        let Some(game) = &self.game else {
            return ScreenBuilder::new("lichess-game")
                .top_bar("Game")
                .activity("Opening the board stream", None)
                .build();
        };
        let elapsed = self.clock.waited().as_secs();
        let white = clock(game.clock_ms(Color::White, elapsed));
        let black = clock(game.clock_ms(Color::Black, elapsed));
        let last = game
            .last_san
            .as_deref()
            .or_else(|| game.state.moves.last().map(String::as_str))
            .unwrap_or("none");
        let mut menu = vec![("confirm-resign".to_owned(), "Resign".to_owned())];
        if game.can_abort() {
            menu.push(("confirm-abort".to_owned(), "Abort".to_owned()));
        }
        let mut screen = ScreenBuilder::new("lichess-game")
            .top_bar(format!("vs {}", game.opponent().name))
            .top_bar_overflow("game-menu", self.menu_open, menu)
            .secondary(format!(
                "White {white}    Black {black} · {}",
                if game.rated { "Rated" } else { "Casual" }
            ))
            .secondary(format!(
                "{} · Last {last}{}",
                if game.my_turn() {
                    "Your move"
                } else {
                    "Opponent's move"
                },
                if game.check { " · Check" } else { "" }
            ));
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if game.takeback_pending() {
            screen = screen.banner(
                BannerLevel::Attention,
                "Takeback controls are not supported. External board changes are reconciled by reopening the stream.",
            );
        }
        if game.opponent_gone {
            screen = screen.banner(
                BannerLevel::Attention,
                self.claim_remaining().map_or_else(
                    || "Opponent disconnected.".to_owned(),
                    |seconds| {
                        if seconds == 0 {
                            "Opponent left; victory may now be claimed.".to_owned()
                        } else {
                            format!("Opponent left; claim available in {seconds}s.")
                        }
                    },
                ),
            );
        }
        screen = screen.board(
            8,
            board_cells(
                &game.fen,
                game.my_color,
                self.selected.as_deref(),
                game.state.moves.last().map(String::as_str),
                chess::checked_king(&game.fen).as_deref(),
            ),
        );
        if !game.active() {
            return screen
                .heading(game.result())
                .primary_button("back-to-play", "Back to Play")
                .build();
        }
        let blocked = self.pending_action.is_some() || self.pending_move.is_some();
        if game.draw_offer_from_opponent() {
            screen = screen
                .button_with_state("accept-draw", "Accept draw", enabled(!blocked))
                .button_with_state("decline-draw", "Decline draw", enabled(!blocked));
        } else {
            screen = screen.button_with_state("offer-draw", "Offer draw", enabled(!blocked));
        }
        if game.opponent_gone && self.claim_remaining() == Some(0) {
            screen = screen.button_with_state("claim-victory", "Claim victory", enabled(!blocked));
        }
        if let Some(promotion) = &self.promotion {
            screen = screen.modal("Choose promotion", |builder| {
                let mut builder = builder.text(format!("{} to {}", promotion.from, promotion.to));
                for choice in &promotion.choices {
                    builder = builder.button(
                        format!("promote-{choice}"),
                        match choice {
                            'q' => "Queen",
                            'r' => "Rook",
                            'b' => "Bishop",
                            'n' => "Knight",
                            _ => "Piece",
                        },
                    );
                }
                builder.button("cancel-promotion", "Cancel")
            });
        } else if let Some(confirmation) = self.confirmation {
            screen = match confirmation {
                Confirmation::Resign => screen.confirm(
                    "Resign game?",
                    "This immediately concedes the rated game.",
                    ("resign", "Resign"),
                    ("cancel-confirm", "Keep playing"),
                ),
                Confirmation::Abort => screen.confirm(
                    "Abort game?",
                    "Abort is offered only before both players have moved; Lichess makes the final decision.",
                    ("abort", "Abort"),
                    ("cancel-confirm", "Keep playing"),
                ),
            };
        } else if let Some(action) = &self.pending_action {
            screen = screen.modal(action.label(), |builder| {
                builder.text("Waiting for Lichess. This request will not be replayed.")
            });
        } else if let Some(movement) = &self.pending_move {
            screen = screen.modal("Waiting for board", |builder| {
                builder.text(format!(
                    "{} was sent. The board will change only after the stream acknowledges it.",
                    movement.movement
                ))
            });
        }
        screen.build()
    }

    fn has_pending(&self, predicate: impl Fn(&Pending) -> bool) -> bool {
        self.tasks.values().any(predicate)
    }

    fn spawn(
        &mut self,
        context: &mut Context,
        pending: Pending,
        work: Task,
        retrying: bool,
    ) -> Option<TaskId> {
        let task = if retrying {
            context.spawn_retrying(work)
        } else {
            context.spawn(work)
        }?;
        self.tasks.insert(task, pending);
        Some(task)
    }

    fn validate_account(&mut self, context: &mut Context) {
        if self.has_pending(|pending| matches!(pending, Pending::Account)) {
            return;
        }
        self.account = AccountState::Checking;
        self.playing_ready = false;
        self.notice = None;
        let _ = self.spawn(context, Pending::Account, api::account(), true);
    }

    fn refresh_playing(&mut self, context: &mut Context) {
        if self.has_pending(|pending| matches!(pending, Pending::Playing)) {
            return;
        }
        let _ = self.spawn(context, Pending::Playing, api::playing(), true);
    }

    fn open_event_stream(&mut self, context: &mut Context) {
        if self.suspended
            || self.event_open
            || self.has_pending(|pending| matches!(pending, Pending::EventOpen))
            || !matches!(self.account, AccountState::Ready(_))
        {
            return;
        }
        let _ = self.spawn(
            context,
            Pending::EventOpen,
            api::event_stream("open"),
            false,
        );
    }

    fn next_event(&mut self, context: &mut Context) {
        if self.suspended
            || !self.event_open
            || self.has_pending(|pending| matches!(pending, Pending::EventNext))
        {
            return;
        }
        let _ = self.spawn(
            context,
            Pending::EventNext,
            api::event_stream("next"),
            false,
        );
    }

    fn close_event(&mut self, context: &mut Context) {
        self.event_open = false;
        if !self.has_pending(|pending| matches!(pending, Pending::EventClose)) {
            let _ = self.spawn(
                context,
                Pending::EventClose,
                api::event_stream("close"),
                false,
            );
        }
    }

    fn schedule_event_reconnect(&mut self, context: &mut Context) {
        self.event_open = false;
        if self.suspended
            || self
                .has_pending(|pending| matches!(pending, Pending::EventOpen | Pending::EventRetry))
            || !matches!(self.account, AccountState::Ready(_))
        {
            return;
        }
        let seconds = self.event_backoff.min(30);
        self.event_backoff = self.event_backoff.saturating_mul(2).min(30);
        let _ = self.spawn(context, Pending::EventRetry, Task::Sleep { seconds }, false);
    }

    fn open_board(&mut self, context: &mut Context, session: Session) {
        if self.suspended {
            return;
        }
        let id = session.game_id.clone();
        if let Some(previous) = self.board_open.clone().filter(|previous| previous != &id) {
            for (task, pending) in self.tasks.clone() {
                if matches!(
                    pending,
                    Pending::BoardOpen(ref open) | Pending::BoardNext(ref open)
                        if open == &previous
                ) {
                    context.cancel(task);
                }
            }
            self.board_open = None;
            self.game = None;
        }
        self.session = Some(session);
        self.persist_session(context);
        if self.board_open.as_deref() == Some(id.as_str())
            || self
                .has_pending(|pending| matches!(pending, Pending::BoardOpen(open) if open == &id))
        {
            return;
        }
        let Some(work) = api::board_stream(&id, "open") else {
            self.notice = Some("Lichess returned an invalid game identifier.".to_owned());
            return;
        };
        self.route = Route::Game;
        let _ = self.spawn(context, Pending::BoardOpen(id), work, false);
        Self::keep_live(context);
    }

    fn next_board(&mut self, context: &mut Context, id: &str) {
        if self.suspended
            || self.board_open.as_deref() != Some(id)
            || self.has_pending(|pending| matches!(pending, Pending::BoardNext(open) if open == id))
        {
            return;
        }
        let Some(work) = api::board_stream(id, "next") else {
            return;
        };
        let _ = self.spawn(context, Pending::BoardNext(id.to_owned()), work, false);
    }

    fn close_board(&mut self, context: &mut Context, id: &str) {
        self.board_open = None;
        if self.has_pending(|pending| matches!(pending, Pending::BoardClose(open) if open == id)) {
            return;
        }
        if let Some(work) = api::board_stream(id, "close") {
            let _ = self.spawn(context, Pending::BoardClose(id.to_owned()), work, false);
        }
    }

    fn schedule_board_reconnect(&mut self, context: &mut Context, id: &str) {
        self.board_open = None;
        if self.suspended
            || self.has_pending(|pending| {
                matches!(pending, Pending::BoardOpen(open) | Pending::BoardRetry(open) if open == id)
            })
        {
            return;
        }
        let seconds = self.board_backoff.min(30);
        self.board_backoff = self.board_backoff.saturating_mul(2).min(30);
        let _ = self.spawn(
            context,
            Pending::BoardRetry(id.to_owned()),
            Task::Sleep { seconds },
            false,
        );
    }

    fn start_seek(&mut self, context: &mut Context) {
        if self.seek_task.is_some() || !self.event_open || !self.playing_ready {
            self.notice = Some(
                "The event stream and current-game snapshot must be ready before pairing."
                    .to_owned(),
            );
            return;
        }
        self.seek_baseline = self
            .summaries
            .iter()
            .map(|game| game.id.clone())
            .chain(self.game.iter().map(|game| game.id.clone()))
            .collect();
        self.seek_waiting = true;
        if let Some(task) = self.spawn(context, Pending::Seek, api::seek(), false) {
            self.seek_task = Some(task);
            self.route = Route::Pairing;
            self.notice = None;
            self.reset_clock(context, true);
            Self::keep_live(context);
        } else {
            self.seek_waiting = false;
            self.seek_baseline.clear();
        }
    }

    fn cancel_seek(&mut self, context: &mut Context) {
        self.seek_waiting = false;
        self.seek_baseline.clear();
        if let Some(task) = self.seek_task {
            context.cancel(task);
            self.notice = Some("Cancelling the pending seek.".to_owned());
        } else if self.route == Route::Pairing {
            self.clock.stop(context);
            self.route = Route::Play;
            self.notice = Some("Pairing cancelled. No duplicate seek was created.".to_owned());
        }
    }

    fn await_seek_event(&mut self, context: &mut Context, message: String) {
        self.seek_task = None;
        self.notice = Some(message);
        if self.seek_waiting && !self.has_pending(|pending| matches!(pending, Pending::SeekGrace)) {
            let _ = self.spawn(
                context,
                Pending::SeekGrace,
                Task::Sleep { seconds: 10 },
                false,
            );
        }
    }

    fn send_action(&mut self, context: &mut Context, action: GameAction, work: Option<Task>) {
        if self.pending_action.is_some() || work.is_none() {
            return;
        }
        if let GameAction::Move(movement) = &action {
            let at_ply = self.game.as_ref().map_or(0, |game| game.state.moves.len());
            self.pending_move = Some(PendingMove {
                movement: movement.clone(),
                at_ply,
            });
        }
        self.pending_action = Some(action.clone());
        if self
            .spawn(
                context,
                Pending::Action(action),
                work.expect("checked"),
                false,
            )
            .is_none()
        {
            self.pending_action = None;
            self.pending_move = None;
            self.notice = Some("Too many requests are already in flight.".to_owned());
        }
    }

    fn choose_game_square(&mut self, context: &mut Context, square: String) {
        if self.pending_action.is_some()
            || self.pending_move.is_some()
            || self.promotion.is_some()
            || self.confirmation.is_some()
        {
            return;
        }
        let Some(game) = &self.game else {
            return;
        };
        if !game.active() {
            return;
        }
        if !game.my_turn() {
            self.notice = Some("Wait for the opponent's move.".to_owned());
            return;
        }
        let Some(from) = self.selected.take() else {
            if chess::piece_belongs_to(&game.fen, &square, game.my_color.fen()) {
                self.selected = Some(square);
                self.notice = None;
            } else {
                self.notice = Some("Select one of your pieces.".to_owned());
            }
            return;
        };
        if from == square {
            self.notice = None;
            return;
        }
        let promotions = chess::promotion_choices(&game.fen, &from, &square);
        if !promotions.is_empty() {
            self.promotion = Some(Promotion {
                from,
                to: square,
                choices: promotions,
            });
            return;
        }
        let movement = format!("{from}{square}");
        if !chess::legal(&game.fen, &movement) {
            self.notice = Some("That move is not legal in the server position.".to_owned());
            return;
        }
        self.send_action(
            context,
            GameAction::Move(movement.clone()),
            api::move_piece(&game.id, &movement),
        );
    }

    fn choose_puzzle_square(&mut self, square: String) {
        let Some(puzzle) = self.puzzles.get_mut(self.current_puzzle) else {
            return;
        };
        let Some(from) = self.selected.take() else {
            self.selected = Some(square);
            self.notice = None;
            return;
        };
        if from == square {
            return;
        }
        let expected = puzzle.solution.get(puzzle.cursor).cloned();
        let mut candidates = expected
            .as_ref()
            .filter(|movement| movement.starts_with(&format!("{from}{square}")))
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        candidates.push(format!("{from}{square}"));
        candidates.extend(
            chess::promotion_choices(&puzzle.fen, &from, &square)
                .into_iter()
                .map(|promotion| format!("{from}{square}{promotion}")),
        );
        let Some(movement) = candidates
            .into_iter()
            .find(|movement| chess::legal(&puzzle.fen, movement))
        else {
            self.notice = Some("That move is not legal here.".to_owned());
            return;
        };
        if expected
            .as_ref()
            .is_some_and(|solution| solution != &movement)
        {
            self.puzzle_wrong = self.puzzle_wrong.saturating_add(1);
            self.notice = Some(if self.puzzle_wrong == 1 {
                "Not it — try again.".to_owned()
            } else {
                format!(
                    "Not solved. The move was {}.",
                    expected.clone().unwrap_or_default()
                )
            });
            if self.puzzle_wrong > 1 {
                self.route = Route::PuzzleResult;
            }
            return;
        }
        if let Some((fen, _)) = chess::play(&puzzle.fen, &movement) {
            puzzle.fen = fen;
            puzzle.cursor = puzzle.cursor.saturating_add(1);
        }
        if puzzle.cursor >= puzzle.solution.len() {
            self.route = Route::PuzzleResult;
        } else {
            self.notice = None;
        }
    }

    fn accept_puzzles(&mut self, bytes: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return false;
        };
        let Some(items) = value
            .as_array()
            .or_else(|| value.get("puzzles").and_then(Value::as_array))
        else {
            return false;
        };
        let puzzles = items
            .iter()
            .filter_map(Puzzle::read)
            .take(MAX_STORED_PUZZLES)
            .collect::<Vec<_>>();
        if puzzles.is_empty() {
            return false;
        }
        self.puzzles = puzzles;
        self.current_puzzle = 0;
        self.puzzle_wrong = 0;
        self.notice = None;
        true
    }

    fn encode_puzzles(&self) -> Vec<u8> {
        Value::Array(self.puzzles.iter().map(Puzzle::stored).collect())
            .to_json()
            .into_bytes()
    }

    fn decode_puzzles(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > 256 * 1024 {
            return false;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return false;
        };
        let Some(items) = value.as_array() else {
            return false;
        };
        let puzzles = items
            .iter()
            .filter_map(Puzzle::from_stored)
            .take(MAX_STORED_PUZZLES)
            .collect::<Vec<_>>();
        if puzzles.is_empty() && !items.is_empty() {
            return false;
        }
        self.puzzles = puzzles;
        self.current_puzzle = 0;
        true
    }

    fn persist_session(&self, context: &mut Context) {
        if let Some(session) = &self.session {
            context.store().save(SESSION_KEY, session.encode());
        }
    }

    fn clear_session(&mut self, context: &mut Context) {
        self.session = None;
        context.store().forget(SESSION_KEY);
    }

    fn maybe_start(&mut self, context: &mut Context) {
        if self.loaded_session && self.loaded_puzzles && self.session.is_some() {
            self.validate_account(context);
        }
    }

    fn account_failure(&mut self, context: &mut Context, error: TaskError) {
        match error {
            TaskError::NoCredential => {
                self.account = AccountState::Missing;
                self.notice = None;
            }
            TaskError::Unauthorized => {
                self.account = AccountState::Invalid;
                self.notice = None;
            }
            other => {
                self.account = AccountState::Failed(Failure::of(other).naming(api::SECRET));
            }
        }
        if let Some(task) = self.seek_task {
            context.cancel(task);
        }
        self.close_live_reads(context);
    }

    fn handle_event(&mut self, context: &mut Context, event: Event) {
        match event {
            Event::GameStart {
                game: summary,
                quick_pair_candidate,
            } => {
                let seek_match = self.seek_waiting
                    && quick_pair_candidate
                    && !self.seek_baseline.contains(&summary.id);
                let accepted_challenge =
                    matches!(self.pending_action, Some(GameAction::AcceptChallenge(_)));
                self.upsert_summary(summary.clone());
                if seek_match || accepted_challenge {
                    if let Some(task) = self.seek_task {
                        context.cancel(task);
                    }
                    self.seek_task = None;
                    self.seek_waiting = false;
                    self.seek_baseline.clear();
                    self.pending_action = None;
                    self.challenge = None;
                    self.notice = Some(if seek_match {
                        "Quick pairing matched. Opening the board.".to_owned()
                    } else {
                        "Challenge accepted. Opening the board.".to_owned()
                    });
                    self.open_board(context, summary.session());
                } else if self.route == Route::Play {
                    self.notice =
                        Some("A Lichess game started. Open it from Ongoing games.".to_owned());
                }
            }
            Event::GameFinish(id) => {
                if self.game.as_ref().is_some_and(|game| game.id == id) {
                    self.notice = Some("Lichess reports that the game finished.".to_owned());
                    if self.board_open.as_deref() == Some(id.as_str()) {
                        self.next_board(context, &id);
                    }
                }
                self.summaries.retain(|summary| summary.id != id);
            }
            Event::Challenge(challenge) => {
                if challenge.direction == ChallengeDirection::Incoming {
                    self.challenge = Some(challenge);
                    if self.route == Route::Play {
                        self.notice = Some("An incoming challenge arrived.".to_owned());
                    }
                }
            }
            Event::ChallengeCanceled(id) | Event::ChallengeDeclined(id) => {
                if self
                    .challenge
                    .as_ref()
                    .is_some_and(|challenge| challenge.id == id)
                {
                    self.challenge = None;
                    self.pending_action = None;
                    if self.route == Route::Challenge {
                        self.route = Route::Play;
                    }
                    self.notice = Some("The challenge is no longer active.".to_owned());
                }
            }
            Event::Unknown => {}
        }
    }

    fn handle_board(&mut self, context: &mut Context, id: &str, record: BoardRecord) {
        match record {
            BoardRecord::Full(full) => {
                let Some(session) = self
                    .session
                    .as_ref()
                    .filter(|session| session.game_id == id)
                else {
                    self.notice = Some("The board did not match the saved game.".to_owned());
                    self.close_board(context, id);
                    return;
                };
                let pending = self.pending_move.clone();
                let Some(game) = Game::from_full(full, session.color) else {
                    self.notice = Some("The server board could not be reconstructed.".to_owned());
                    self.close_board(context, id);
                    self.schedule_board_reconnect(context, id);
                    return;
                };
                self.game = Some(game);
                self.board_backoff = 1;
                self.reconcile_pending_move();
                if pending.is_some() && self.pending_move.is_none() {
                    self.notice = Some("The board reconciled the pending move.".to_owned());
                } else if pending.is_some() {
                    self.pending_move = None;
                    self.pending_action = None;
                    self.selected = None;
                    self.notice = Some(
                        "The move was absent from the authoritative reconnect state and was not replayed."
                            .to_owned(),
                    );
                }
                self.reset_clock(context, self.game.as_ref().is_some_and(Game::active));
                self.finish_if_needed(context);
            }
            BoardRecord::State(state) => {
                let Some(game) = self.game.as_mut().filter(|game| game.id == id) else {
                    self.notice = Some("Waiting for the complete board state.".to_owned());
                    self.close_board(context, id);
                    self.schedule_board_reconnect(context, id);
                    return;
                };
                let applied = game.apply(state);
                let active = game.active();
                match applied {
                    Some(ApplyState::Changed) => {
                        self.pending_action = None;
                        self.reconcile_pending_move();
                        self.notice = None;
                        self.reset_clock(context, active);
                        self.finish_if_needed(context);
                    }
                    Some(ApplyState::Unchanged) => {}
                    Some(ApplyState::Reopen) => {
                        self.notice = Some(
                            "Board history moved backward or diverged; reopening authoritative state."
                                .to_owned(),
                        );
                        self.close_board(context, id);
                        self.schedule_board_reconnect(context, id);
                    }
                    None => {
                        self.notice =
                            Some("Lichess sent a move list that could not be replayed.".to_owned());
                        self.close_board(context, id);
                        self.schedule_board_reconnect(context, id);
                    }
                }
            }
            BoardRecord::OpponentGone {
                gone,
                claim_win_seconds,
            } => {
                if let Some(game) = self.game.as_mut().filter(|game| game.id == id) {
                    game.opponent_gone = gone;
                    game.claim_win_seconds = claim_win_seconds;
                    self.gone_tick = self.total_ticks;
                }
            }
            BoardRecord::Ignored => {}
        }
    }

    fn reconcile_pending_move(&mut self) {
        let Some(pending) = self.pending_move.clone() else {
            return;
        };
        let Some(game) = &self.game else {
            return;
        };
        if game.state.moves.len() <= pending.at_ply {
            return;
        }
        if game.state.moves.get(pending.at_ply) == Some(&pending.movement) {
            self.pending_move = None;
            self.pending_action = None;
            self.selected = None;
        } else {
            self.pending_move = None;
            self.pending_action = None;
            self.selected = None;
            self.notice = Some(
                "The server advanced without that move; the displayed board is authoritative."
                    .to_owned(),
            );
        }
    }

    fn finish_if_needed(&mut self, context: &mut Context) {
        let Some(game) = &self.game else {
            return;
        };
        if game.active() {
            return;
        }
        let id = game.id.clone();
        self.pending_action = None;
        self.pending_move = None;
        self.selected = None;
        self.clock.stop(context);
        self.close_board(context, &id);
        self.clear_session(context);
        self.summaries.retain(|summary| summary.id != id);
        self.route = Route::Game;
    }

    fn upsert_summary(&mut self, summary: GameSummary) {
        if let Some(existing) = self
            .summaries
            .iter_mut()
            .find(|existing| existing.id == summary.id)
        {
            *existing = summary;
        } else {
            self.summaries.push(summary);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "every asynchronous Board API operation is completed in one exhaustive state transition"
    )]
    fn handle_completed(&mut self, context: &mut Context, pending: Pending, bytes: &[u8]) {
        if let Some(api::RateLimit::Limited(delay)) = api::rate_limit(bytes) {
            self.notice = Some(delay.map_or_else(
                || "Lichess rate-limited this request. Try again later.".to_owned(),
                |seconds| format!("Lichess rate-limited this request. Try again in {seconds}s."),
            ));
            match pending {
                Pending::Account => {
                    self.account = AccountState::Failed(self.notice.clone().unwrap());
                }
                Pending::EventOpen | Pending::EventNext => self.event_open = false,
                Pending::BoardOpen(_) | Pending::BoardNext(_) => self.board_open = None,
                Pending::Seek => {
                    self.seek_task = None;
                    self.seek_waiting = false;
                    self.seek_baseline.clear();
                    self.clock.stop(context);
                    self.route = Route::Play;
                }
                Pending::Action(_) => {
                    self.pending_action = None;
                    self.pending_move = None;
                }
                _ => {}
            }
            return;
        }
        match pending {
            Pending::Account => {
                if let Some(account) = api::parse_account(bytes) {
                    self.account = AccountState::Ready(account);
                    self.event_backoff = 1;
                    self.open_event_stream(context);
                    self.refresh_playing(context);
                } else {
                    self.account = AccountState::Failed(
                        "Lichess returned an account response this client cannot read.".to_owned(),
                    );
                }
            }
            Pending::Playing => {
                if let Some(games) = api::parse_playing(bytes) {
                    self.playing_ready = true;
                    self.summaries = games;
                    if let Some(session) = self.session.clone() {
                        if let Some(summary) = self
                            .summaries
                            .iter()
                            .find(|summary| summary.id == session.game_id)
                            .cloned()
                        {
                            self.open_board(context, summary.session());
                        } else {
                            self.notice = Some(
                                "The saved game is no longer active; stale reconnect state was cleared."
                                    .to_owned(),
                            );
                            self.clear_session(context);
                        }
                    }
                } else {
                    self.playing_ready = false;
                    self.notice =
                        Some("Lichess returned a game list this client cannot read.".to_owned());
                }
            }
            Pending::Puzzle => {
                if self.accept_puzzles(bytes) {
                    context.store().save(PUZZLE_KEY, self.encode_puzzles());
                    self.route = Route::Puzzles;
                } else {
                    self.notice =
                        Some("Lichess returned a puzzle batch this client cannot read.".to_owned());
                }
            }
            Pending::EventOpen => {
                if bytes.is_empty() {
                    self.event_open = true;
                    self.event_backoff = 1;
                    self.next_event(context);
                } else {
                    self.notice = Some("The event stream did not open cleanly.".to_owned());
                }
            }
            Pending::EventNext => {
                if let Some(event) = api::parse_event(bytes) {
                    self.handle_event(context, event);
                    self.next_event(context);
                } else {
                    self.notice = Some(
                        "The event stream record was malformed; reopening without replay."
                            .to_owned(),
                    );
                    self.close_event(context);
                }
            }
            Pending::EventRetry => self.open_event_stream(context),
            Pending::EventClose => self.schedule_event_reconnect(context),
            Pending::Seek => {
                if self.route == Route::Pairing && self.seek_waiting {
                    self.await_seek_event(
                        context,
                        "The seek connection ended; waiting briefly for gameStart. It was not replayed."
                            .to_owned(),
                    );
                } else {
                    self.seek_task = None;
                }
            }
            Pending::SeekGrace => {
                if self.seek_waiting {
                    self.seek_waiting = false;
                    self.seek_baseline.clear();
                    self.clock.stop(context);
                    self.route = Route::Play;
                    self.notice = Some(
                        "No gameStart followed the ended seek. It was not replayed.".to_owned(),
                    );
                }
            }
            Pending::BoardOpen(id) => {
                if bytes.is_empty() {
                    self.board_open = Some(id.clone());
                    self.board_backoff = 1;
                    self.next_board(context, &id);
                } else {
                    self.notice = Some("The board stream did not open cleanly.".to_owned());
                }
            }
            Pending::BoardNext(id) => {
                if let Some(record) = api::parse_board(bytes, &id) {
                    self.handle_board(context, &id, record);
                    if self.game.as_ref().is_some_and(Game::active) {
                        self.next_board(context, &id);
                    }
                } else {
                    self.notice = Some(
                        "The board stream record was malformed; reopening authoritative state."
                            .to_owned(),
                    );
                    self.close_board(context, &id);
                    self.schedule_board_reconnect(context, &id);
                }
            }
            Pending::BoardRetry(id) => {
                if let Some(session) = self.session.clone().filter(|session| session.game_id == id)
                {
                    self.open_board(context, session);
                }
            }
            Pending::BoardClose(_) => {}
            Pending::Action(action) => {
                self.pending_action = match action {
                    GameAction::Move(_) => None,
                    GameAction::DeclineChallenge(id) => {
                        if self
                            .challenge
                            .as_ref()
                            .is_some_and(|challenge| challenge.id == id)
                        {
                            self.challenge = None;
                            self.route = Route::Play;
                        }
                        None
                    }
                    GameAction::AcceptChallenge(_) if self.route == Route::Game => None,
                    _other if self.game.as_ref().is_some_and(|game| !game.active()) => None,
                    other => Some(other),
                };
                self.notice = Some(
                    "Lichess accepted the request; waiting for the stream to confirm it."
                        .to_owned(),
                );
            }
        }
    }

    fn handle_failed(&mut self, context: &mut Context, pending: Pending, error: TaskError) {
        if matches!(pending, Pending::Account) {
            self.account_failure(context, error);
            return;
        }
        if matches!(error, TaskError::NoCredential | TaskError::Unauthorized) {
            self.account_failure(context, error);
            return;
        }
        match pending {
            Pending::EventOpen | Pending::EventNext => {
                self.notice = Some(Failure::of(error).naming(api::SECRET));
                self.schedule_event_reconnect(context);
            }
            Pending::BoardOpen(id) | Pending::BoardNext(id) => {
                if self
                    .session
                    .as_ref()
                    .is_none_or(|session| session.game_id != id)
                {
                    return;
                }
                if error == TaskError::NotFound {
                    self.notice = Some(
                        "The saved game is no longer available; reconnect state was cleared."
                            .to_owned(),
                    );
                    self.clear_session(context);
                    self.game = None;
                    self.route = Route::Play;
                } else {
                    self.notice = Some(Failure::of(error).naming(api::SECRET));
                    self.schedule_board_reconnect(context, &id);
                }
            }
            Pending::Seek => {
                if self.route == Route::Pairing && self.seek_waiting {
                    self.await_seek_event(
                        context,
                        format!(
                            "{} Waiting briefly for gameStart; the seek was not replayed.",
                            Failure::of(error).naming(api::SECRET)
                        ),
                    );
                } else {
                    self.seek_task = None;
                }
            }
            Pending::Action(action) => {
                self.pending_action = None;
                self.notice = Some(format!(
                    "{} The action was not replayed; the board is being reconciled.",
                    Failure::of(error).naming(api::SECRET)
                ));
                if !matches!(action, GameAction::Move(_)) {
                    self.pending_move = None;
                }
                if let Some(id) = self.game.as_ref().map(|game| game.id.clone()) {
                    self.close_board(context, &id);
                    self.schedule_board_reconnect(context, &id);
                }
            }
            Pending::Puzzle => {
                self.notice = Some(Failure::of(error).advice.to_owned());
            }
            Pending::Playing => {
                self.notice = Some(Failure::of(error).naming(api::SECRET));
            }
            Pending::EventRetry
            | Pending::EventClose
            | Pending::SeekGrace
            | Pending::BoardRetry(_)
            | Pending::BoardClose(_)
            | Pending::Account => {}
        }
    }

    fn handle_cancelled(&mut self, context: &mut Context, pending: &Pending) {
        match pending {
            Pending::Seek => {
                self.seek_task = None;
                self.seek_waiting = false;
                self.seek_baseline.clear();
                if self.route == Route::Pairing {
                    self.clock.stop(context);
                    self.route = Route::Play;
                    self.notice =
                        Some("Pairing cancelled. No duplicate seek was created.".to_owned());
                }
            }
            Pending::EventNext | Pending::EventOpen => self.event_open = false,
            Pending::BoardNext(id) | Pending::BoardOpen(id)
                if self.board_open.as_deref() == Some(id) =>
            {
                self.board_open = None;
            }
            Pending::Action(_) => {
                self.pending_action = None;
                self.notice = Some(
                    "The action was cancelled with an unknown server outcome; reconnecting."
                        .to_owned(),
                );
            }
            _ => {}
        }
    }

    fn reset_clock(&mut self, context: &mut Context, running: bool) {
        self.clock.stop(context);
        if running {
            self.clock.start(context);
        }
    }

    fn keep_live(context: &mut Context) {
        context.device().hold_wifi(Duration::from_secs(10 * 60));
        context.device().keep_awake(Duration::from_secs(60 * 60));
    }

    fn claim_remaining(&self) -> Option<u32> {
        let game = self.game.as_ref()?;
        let seconds = game.claim_win_seconds?;
        let elapsed = self.total_ticks.saturating_sub(self.gone_tick);
        Some(seconds.saturating_sub(u32::try_from(elapsed).unwrap_or(u32::MAX)))
    }

    fn remaining_puzzles(&self) -> usize {
        self.puzzles.len().saturating_sub(self.current_puzzle)
    }

    fn close_live_reads(&mut self, context: &mut Context) {
        for (task, pending) in self.tasks.clone() {
            if matches!(
                pending,
                Pending::EventOpen
                    | Pending::EventNext
                    | Pending::BoardOpen(_)
                    | Pending::BoardNext(_)
                    | Pending::Seek
            ) {
                context.cancel(task);
            }
        }
        self.seek_task = None;
        self.seek_waiting = false;
        self.seek_baseline.clear();
        self.event_open = false;
        self.board_open = None;
        self.clock.stop(context);
    }

    #[cfg(debug_assertions)]
    fn install_demo(&mut self, scenario: &str) -> bool {
        match scenario {
            "home" => {
                self.route = Route::Home;
            }
            "game" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.session = Some(Session {
                    game_id: "demoAB12".to_owned(),
                    color: Color::Black,
                    opponent: "KnightReader".to_owned(),
                    rated: true,
                });
                self.game = Game::from_full(
                    FullGame {
                        id: "demoAB12".to_owned(),
                        initial_fen: "startpos".to_owned(),
                        rated: true,
                        speed: "rapid".to_owned(),
                        white: Player {
                            id: Some("other".to_owned()),
                            name: "KnightReader".to_owned(),
                            rating: Some(1542),
                        },
                        black: Player {
                            id: Some("demo-owner".to_owned()),
                            name: "DemoOwner".to_owned(),
                            rating: Some(1510),
                        },
                        state: ServerState {
                            moves: ["e2e4", "c7c5", "g1f3"]
                                .into_iter()
                                .map(str::to_owned)
                                .collect(),
                            white_ms: 574_000,
                            black_ms: 590_000,
                            white_increment_ms: 0,
                            black_increment_ms: 0,
                            status: "started".to_owned(),
                            winner: None,
                            white_draw: false,
                            black_draw: false,
                            white_takeback: false,
                            black_takeback: false,
                        },
                    },
                    Color::Black,
                );
                self.route = Route::Game;
            }
            "pairing" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.event_open = true;
                self.seek_task = Some(TaskId(999));
                self.route = Route::Pairing;
            }
            "challenge" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.challenge = Some(Challenge {
                    id: "chall123".to_owned(),
                    challenger: "KnightReader".to_owned(),
                    direction: ChallengeDirection::Incoming,
                    status: "created".to_owned(),
                    rated: false,
                    variant: "standard".to_owned(),
                    speed: "rapid".to_owned(),
                    time_control: ChallengeTime::Clock {
                        initial_seconds: Some(600),
                        increment_seconds: Some(0),
                    },
                });
                self.route = Route::Challenge;
            }
            "missing" => {
                self.account = AccountState::Missing;
                self.route = Route::Play;
            }
            _ => return false,
        }
        self.loaded_session = true;
        self.loaded_puzzles = true;
        true
    }

    #[cfg(not(debug_assertions))]
    fn install_demo(&mut self, _scenario: &str) -> bool {
        false
    }
}

impl KoboApp for Lichess {
    fn on_start(&mut self, context: &mut Context) {
        if std::env::var("KOBO_LICHESS_DEMO")
            .ok()
            .is_some_and(|scenario| self.install_demo(&scenario))
        {
            if self.game.as_ref().is_some_and(Game::active) || self.route == Route::Pairing {
                self.reset_clock(context, true);
            }
            self.show(context);
            return;
        }
        context.store().load(SESSION_KEY);
        context.store().load(PUZZLE_KEY);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == SESSION_KEY {
                self.loaded_session = true;
                self.session = value.as_deref().and_then(Session::decode);
                if value.is_some() && self.session.is_none() {
                    context.store().forget(SESSION_KEY);
                    self.notice =
                        Some("Corrupted reconnect state was discarded safely.".to_owned());
                }
            } else if key == PUZZLE_KEY {
                self.loaded_puzzles = true;
                if let Some(bytes) = value {
                    if !self.decode_puzzles(&bytes) {
                        context.store().forget(PUZZLE_KEY);
                        self.notice = Some("Corrupted puzzle state was discarded.".to_owned());
                    }
                }
            }
            self.maybe_start(context);
            self.show(context);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "action dispatch is a flat exhaustive mapping from stable UI identifiers to state transitions"
    )]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            if self.promotion.take().is_some()
                || self.confirmation.take().is_some()
                || self.menu_open
            {
                self.menu_open = false;
            } else if self.route == Route::Pairing
                && (self.seek_task.is_some() || self.seek_waiting)
            {
                self.cancel_seek(context);
            } else {
                self.selected = None;
                self.route = match self.route {
                    Route::Solve | Route::PuzzleResult => Route::Puzzles,
                    Route::Game | Route::Pairing | Route::Challenge => Route::Play,
                    _ => Route::Home,
                };
            }
        } else if action == action_id("puzzles") {
            self.route = Route::Puzzles;
        } else if action == action_id("play") {
            self.route = Route::Play;
            self.validate_account(context);
        } else if action == action_id("refresh-play") {
            self.validate_account(context);
        } else if action == action_id("quick-pair") {
            self.start_seek(context);
        } else if action == action_id("cancel-seek") {
            self.cancel_seek(context);
        } else if action == action_id("open-challenge") {
            self.route = Route::Challenge;
        } else if action == action_id("accept-challenge") {
            if let Some(challenge) = self.challenge.clone().filter(Challenge::supported) {
                self.send_action(
                    context,
                    GameAction::AcceptChallenge(challenge.id.clone()),
                    api::challenge(&challenge.id, true),
                );
            }
        } else if action == action_id("decline-challenge") {
            if let Some(challenge) = self.challenge.clone() {
                self.send_action(
                    context,
                    GameAction::DeclineChallenge(challenge.id.clone()),
                    api::challenge(&challenge.id, false),
                );
            }
        } else if action == action_id("resume-current") {
            self.route = Route::Game;
        } else if let Some(index) = indexed_action(action, "resume-", self.summaries.len()) {
            if let Some(summary) = self.summaries.get(index).cloned() {
                self.open_board(context, summary.session());
            }
        } else if action == action_id("game-menu") {
            self.menu_open = !self.menu_open;
        } else if action == action_id("confirm-resign") {
            self.menu_open = false;
            self.confirmation = Some(Confirmation::Resign);
        } else if action == action_id("confirm-abort") {
            self.menu_open = false;
            if self.game.as_ref().is_some_and(Game::can_abort) {
                self.confirmation = Some(Confirmation::Abort);
            }
        } else if action == action_id("cancel-confirm") {
            self.confirmation = None;
        } else if action == action_id("resign") {
            self.confirmation = None;
            if let Some(game) = &self.game {
                self.send_action(context, GameAction::Resign, api::resign(&game.id));
            }
        } else if action == action_id("abort") {
            self.confirmation = None;
            if let Some(game) = &self.game {
                self.send_action(context, GameAction::Abort, api::abort(&game.id));
            }
        } else if action == action_id("offer-draw") {
            if let Some(game) = &self.game {
                self.send_action(context, GameAction::OfferDraw, api::draw(&game.id, true));
            }
        } else if action == action_id("accept-draw") {
            if let Some(game) = &self.game {
                self.send_action(context, GameAction::AcceptDraw, api::draw(&game.id, true));
            }
        } else if action == action_id("decline-draw") {
            if let Some(game) = &self.game {
                self.send_action(context, GameAction::DeclineDraw, api::draw(&game.id, false));
            }
        } else if action == action_id("claim-victory") && self.claim_remaining() == Some(0) {
            if let Some(game) = &self.game {
                self.send_action(
                    context,
                    GameAction::ClaimVictory,
                    api::claim_victory(&game.id),
                );
            }
        } else if action == action_id("cancel-promotion") {
            self.promotion = None;
        } else if let Some(choice) = promotion_action(action) {
            if let (Some(promotion), Some(game)) = (self.promotion.take(), self.game.as_ref()) {
                if promotion.choices.contains(&choice) {
                    let movement = format!("{}{}{}", promotion.from, promotion.to, choice);
                    self.send_action(
                        context,
                        GameAction::Move(movement.clone()),
                        api::move_piece(&game.id, &movement),
                    );
                }
            }
        } else if action == action_id("back-to-play") {
            self.route = Route::Play;
            self.refresh_playing(context);
        } else if action == action_id("new-puzzles") {
            if !self.has_pending(|pending| matches!(pending, Pending::Puzzle)) {
                let _ = self.spawn(context, Pending::Puzzle, api::puzzle(false), true);
                self.notice = Some("Downloading a deterministic-size puzzle batch.".to_owned());
            }
        } else if action == action_id("solve") && self.remaining_puzzles() > 0 {
            self.route = Route::Solve;
        } else if action == action_id("skip-puzzle") {
            self.puzzle_wrong = 2;
            self.route = Route::PuzzleResult;
        } else if action == action_id("next-puzzle") {
            self.current_puzzle = self.current_puzzle.saturating_add(1);
            self.puzzle_wrong = 0;
            self.selected = None;
            self.route = if self.remaining_puzzles() > 0 {
                Route::Solve
            } else {
                Route::Puzzles
            };
            context.store().save(PUZZLE_KEY, self.encode_puzzles());
        } else if let Some(square) = square_action(action) {
            match self.route {
                Route::Game => self.choose_game_square(context, square),
                Route::Solve => self.choose_puzzle_square(square),
                _ => {}
            }
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.clock.on_task(context, task, &outcome) {
            if !matches!(outcome, TaskOutcome::Cancelled) {
                self.total_ticks = self.total_ticks.saturating_add(1);
                if self.total_ticks % (8 * 60) == 0
                    && (self.game.as_ref().is_some_and(Game::active) || self.seek_task.is_some())
                {
                    Self::keep_live(context);
                }
            }
            self.show(context);
            return;
        }
        let Some(pending) = self.tasks.remove(&task) else {
            return;
        };
        match outcome {
            TaskOutcome::Completed(bytes) => self.handle_completed(context, pending, &bytes),
            TaskOutcome::Failed(error) => self.handle_failed(context, pending, error),
            TaskOutcome::Cancelled => self.handle_cancelled(context, &pending),
        }
        self.show(context);
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.suspended = true;
        self.close_live_reads(context);
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.suspended = false;
        self.validate_account(context);
        self.show(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.suspended = true;
        self.close_live_reads(context);
    }
}

fn enabled(enabled: bool) -> ControlState {
    if enabled {
        ControlState::Enabled
    } else {
        ControlState::Disabled
    }
}

fn clock(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn board_cells(
    fen: &str,
    orientation: Color,
    selected: Option<&str>,
    last_move: Option<&str>,
    checked_king: Option<&str>,
) -> Vec<(String, String, Option<Glyph>)> {
    let last_from = last_move.and_then(|movement| movement.get(0..2));
    let last_to = last_move.and_then(|movement| movement.get(2..4));
    let ranks: Vec<u8> = match orientation {
        Color::White => (1..=8).rev().collect(),
        Color::Black => (1..=8).collect(),
    };
    let files: Vec<u8> = match orientation {
        Color::White => (b'a'..=b'h').collect(),
        Color::Black => (b'a'..=b'h').rev().collect(),
    };
    let mut cells = Vec::with_capacity(64);
    for rank in ranks {
        for file in &files {
            let square = format!("{}{}", char::from(*file), rank);
            let piece = chess::piece_at(fen, &square);
            let mut label = piece.map_or_else(|| " ".to_owned(), piece_label);
            if selected == Some(square.as_str()) {
                label.push('*');
            } else if checked_king == Some(square.as_str()) {
                label.push('+');
            } else if last_to == Some(square.as_str()) {
                label.push('!');
            } else if last_from == Some(square.as_str()) && piece.is_none() {
                label.clear();
                label.push_str("..");
            }
            cells.push((format!("square-{square}"), label, None));
        }
    }
    cells
}

fn piece_label(piece: char) -> String {
    match piece {
        'K' => "wK",
        'Q' => "wQ",
        'R' => "wR",
        'B' => "wB",
        'N' => "wN",
        'P' => "wP",
        'k' => "bK",
        'q' => "bQ",
        'r' => "bR",
        'b' => "bB",
        'n' => "bN",
        'p' => "bP",
        _ => " ",
    }
    .to_owned()
}

fn square_action(action: ActionId) -> Option<String> {
    for file in b'a'..=b'h' {
        for rank in 1..=8 {
            let square = format!("{}{}", char::from(file), rank);
            if action == action_id(&format!("square-{square}")) {
                return Some(square);
            }
        }
    }
    None
}

fn indexed_action(action: ActionId, prefix: &str, count: usize) -> Option<usize> {
    (0..count).find(|index| action == action_id(&format!("{prefix}{index}")))
}

fn promotion_action(action: ActionId) -> Option<char> {
    ['q', 'r', 'b', 'n']
        .into_iter()
        .find(|choice| action == action_id(&format!("promote-{choice}")))
}

fn valid_move(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.len(), 4 | 5)
        && matches!(bytes[0], b'a'..=b'h')
        && matches!(bytes[1], b'1'..=b'8')
        && matches!(bytes[2], b'a'..=b'h')
        && matches!(bytes[3], b'1'..=b'8')
        && (bytes.len() == 4 || matches!(bytes[4], b'q' | b'r' | b'b' | b'n'))
}

fn main() -> ExitCode {
    match kobo_sdk::run("lichess", Lichess::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lichess: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        api, board_cells, clock, AccountState, Color, FullGame, Game, GameAction, Lichess, Pending,
        Player, Route, ServerState, Session,
    };
    use kobo_sdk::{action_id, ActionId, Command, Context, KoboApp, TaskOutcome};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn live_game(moves: &[&str], color: Color) -> Game {
        Game::from_full(
            FullGame {
                id: "abcdEF12".to_owned(),
                initial_fen: "startpos".to_owned(),
                rated: true,
                speed: "rapid".to_owned(),
                white: Player {
                    id: Some("owner123".to_owned()),
                    name: "Owner".to_owned(),
                    rating: Some(1500),
                },
                black: Player {
                    id: Some("other123".to_owned()),
                    name: "Other".to_owned(),
                    rating: Some(1510),
                },
                state: ServerState {
                    moves: moves
                        .iter()
                        .map(|movement| (*movement).to_owned())
                        .collect(),
                    white_ms: 600_000,
                    black_ms: 600_000,
                    white_increment_ms: 0,
                    black_increment_ms: 0,
                    status: "started".to_owned(),
                    winner: None,
                    white_draw: false,
                    black_draw: false,
                    white_takeback: false,
                    black_takeback: false,
                },
            },
            color,
        )
        .expect("game")
    }

    fn app_with_game(moves: &[&str], color: Color) -> Lichess {
        let game = live_game(moves, color);
        Lichess {
            route: Route::Game,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            session: Some(Session {
                game_id: game.id.clone(),
                color,
                opponent: "Other".to_owned(),
                rated: true,
            }),
            game: Some(game),
            ..Lichess::default()
        }
    }

    #[test]
    fn black_orientation_and_live_controls_fit_clara_bw() {
        let app = app_with_game(&["e2e4", "c7c5", "g1f3"], Color::Black);
        let screen = app.game_screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for square in ["a1", "e4", "h8"] {
            assert!(layout
                .rect_of_action(action_id(&format!("square-{square}")))
                .is_some());
        }
        let cells = board_cells(
            &app.game.as_ref().expect("game").fen,
            Color::Black,
            None,
            Some("g1f3"),
            None,
        );
        assert_eq!(cells.first().expect("first").0, "square-h1");
        assert_eq!(cells.last().expect("last").0, "square-a8");
    }

    #[test]
    fn a_move_never_changes_local_board_before_stream_acknowledgement() {
        let mut app = app_with_game(&[], Color::White);
        let before = app.game.as_ref().expect("game").fen.clone();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e4"));
        assert_eq!(app.game.as_ref().expect("game").fen, before);
        assert_eq!(
            app.pending_move
                .as_ref()
                .map(|pending| pending.movement.as_str()),
            Some("e2e4")
        );
        let posts = context
            .commands()
            .iter()
            .filter(|command| matches!(command, Command::Spawn { .. }))
            .count();
        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e4"));
        assert_eq!(
            context
                .commands()
                .iter()
                .filter(|command| matches!(command, Command::Spawn { .. }))
                .count(),
            posts,
            "a pending move was submitted twice"
        );
    }

    #[test]
    fn move_acknowledgement_comes_only_from_board_state() {
        let mut app = app_with_game(&[], Color::White);
        app.pending_move = Some(super::PendingMove {
            movement: "e2e4".to_owned(),
            at_ply: 0,
        });
        app.pending_action = Some(GameAction::Move("e2e4".to_owned()));
        let mut context = Context::default();
        app.handle_board(
            &mut context,
            "abcdEF12",
            api::BoardRecord::State(ServerState {
                moves: vec!["e2e4".to_owned()],
                black_ms: 599_000,
                ..app.game.as_ref().expect("game").state.clone()
            }),
        );
        assert!(app.pending_move.is_none());
        assert!(app.pending_action.is_none());
        assert_eq!(
            app.game.as_ref().expect("game").state.moves,
            ["e2e4".to_owned()]
        );
    }

    #[test]
    fn seek_is_spawned_once_and_never_as_retrying_work() {
        let mut app = Lichess {
            route: Route::Play,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.on_action(&mut context, action_id("quick-pair"));
        app.on_action(&mut context, action_id("quick-pair"));
        let seeks = context
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::Spawn { work, .. }
                    if matches!(work, kobo_sdk::Task::Post { url, .. } if url.ends_with("/api/board/seek")) =>
                {
                    Some(work)
                }
                _ => None,
            })
            .count();
        assert_eq!(seeks, 1);
    }

    #[test]
    fn account_global_game_starts_do_not_hijack_a_pending_seek() {
        let mut app = Lichess {
            route: Route::Play,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            summaries: vec![super::GameSummary {
                id: "oldGame1".to_owned(),
                color: Color::White,
                opponent: "Existing".to_owned(),
                rated: true,
                is_my_turn: false,
                last_move: Some("e2e4".to_owned()),
            }],
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.start_seek(&mut context);
        let seek = app.seek_task;
        let replayed = api::parse_event(
            br#"{"type":"gameStart","game":{"id":"oldGame1","color":"white","rated":true,"speed":"rapid","source":"lobby","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":false,"lastMove":"e2e4","opponent":{"username":"Existing"}}}"#,
        )
        .expect("replayed game");
        app.handle_event(&mut context, replayed);
        assert_eq!(app.seek_task, seek);
        assert_eq!(app.route, Route::Pairing);

        let unrelated = api::parse_event(
            br#"{"type":"gameStart","game":{"id":"friend12","color":"black","rated":true,"speed":"rapid","source":"friend","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":false,"lastMove":"","opponent":{"username":"Friend"}}}"#,
        )
        .expect("unrelated game");
        app.handle_event(&mut context, unrelated);
        assert_eq!(app.seek_task, seek);
        assert_eq!(app.route, Route::Pairing);
    }

    #[test]
    fn outgoing_and_non_clock_challenges_never_offer_invalid_actions() {
        let mut app = Lichess {
            route: Route::Play,
            ..Lichess::default()
        };
        let mut context = Context::default();
        let outgoing = api::parse_event(
            br#"{"type":"challenge","challenge":{"id":"other123","status":"created","direction":"out","challenger":{"name":"Owner"},"rated":false,"variant":{"key":"standard"},"speed":"correspondence","timeControl":{"type":"unlimited"}}}"#,
        )
        .expect("outgoing challenge");
        app.handle_event(&mut context, outgoing);
        assert!(app.challenge.is_none());

        let correspondence = api::parse_event(
            br#"{"type":"challenge","challenge":{"id":"days1234","status":"created","direction":"in","challenger":{"name":"Friend"},"rated":false,"variant":{"key":"standard"},"speed":"correspondence","timeControl":{"type":"correspondence","daysPerTurn":3}}}"#,
        )
        .expect("correspondence challenge");
        app.handle_event(&mut context, correspondence);
        assert!(app.challenge.is_some());
        assert!(!app.challenge.as_ref().expect("challenge").supported());
        assert!(format!("{:?}", app.challenge_screen()).contains("standard clock games only"));
    }

    #[test]
    fn seek_cancellation_arriving_after_game_start_does_not_stop_game_clock() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.clock.start(&mut context);
        assert!(app.clock.is_running());
        app.handle_cancelled(&mut context, &Pending::Seek);
        assert!(app.clock.is_running());
        assert_eq!(app.route, Route::Game);
    }

    #[test]
    fn back_from_pairing_cancels_before_leaving_the_screen() {
        let mut app = Lichess {
            route: Route::Play,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.start_seek(&mut context);
        let seek = app.seek_task.expect("seek");
        app.on_action(&mut context, ActionId::BACK);
        assert_eq!(app.route, Route::Pairing);
        assert!(!app.seek_waiting);
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Cancel(task) if *task == seek)));
        app.handle_cancelled(&mut context, &Pending::Seek);
        assert_eq!(app.route, Route::Play);
    }

    #[test]
    fn game_start_can_win_the_race_after_the_seek_connection_ends() {
        let mut app = Lichess {
            route: Route::Play,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.start_seek(&mut context);
        app.handle_failed(
            &mut context,
            Pending::Seek,
            kobo_sdk::TaskError::Unreachable,
        );
        assert!(app.seek_waiting);
        assert_eq!(app.route, Route::Pairing);
        let started = api::parse_event(
            br#"{"type":"gameStart","game":{"gameId":"abcdEF12","color":"white","rated":true,"speed":"rapid","source":"lobby","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"Other"}}}"#,
        )
        .expect("matched game");
        app.handle_event(&mut context, started);
        assert!(!app.seek_waiting);
        assert_eq!(app.route, Route::Game);
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("abcdEF12")
        );
    }

    #[test]
    fn unusable_board_state_closes_the_retained_stream_before_retrying() {
        let mut app = Lichess {
            route: Route::Game,
            session: Some(Session {
                game_id: "abcdEF12".to_owned(),
                color: Color::White,
                opponent: "Other".to_owned(),
                rated: true,
            }),
            board_open: Some("abcdEF12".to_owned()),
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.handle_board(
            &mut context,
            "abcdEF12",
            api::BoardRecord::State(ServerState {
                moves: Vec::new(),
                white_ms: 600_000,
                black_ms: 600_000,
                white_increment_ms: 0,
                black_increment_ms: 0,
                status: "started".to_owned(),
                winner: None,
                white_draw: false,
                black_draw: false,
                white_takeback: false,
                black_takeback: false,
            }),
        );
        assert!(context.commands().iter().any(|command| {
            matches!(
                command,
                Command::Spawn {
                    work: kobo_sdk::Task::Fetch { headers, .. },
                    ..
                } if headers.iter().any(|header| {
                    header.name.eq_ignore_ascii_case("x-cobalt-line-stream")
                        && header.value == "close"
                })
            )
        }));
    }

    #[test]
    fn auth_and_rate_limit_guidance_never_echoes_a_token() {
        let mut app = Lichess::default();
        let mut context = Context::default();
        app.tasks.insert(kobo_sdk::TaskId(7), Pending::Account);
        app.on_task(
            &mut context,
            kobo_sdk::TaskId(7),
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
        );
        assert!(matches!(app.account, AccountState::Missing));
        assert!(app.notice.is_none());
        assert!(format!("{:?}", app.play_screen()).contains("No secret named lichess"));
        app.tasks.insert(kobo_sdk::TaskId(8), Pending::Account);
        app.on_task(
            &mut context,
            kobo_sdk::TaskId(8),
            TaskOutcome::Completed(b"COBALT-HTTP/1 429\nRetry-After: 17\n\n".to_vec()),
        );
        assert!(app.notice.as_deref().unwrap_or_default().contains("17s"));
    }

    #[test]
    fn deterministic_mock_e2e_pairs_moves_reconnects_and_finishes_by_draw() {
        let mut app = Lichess {
            route: Route::Play,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        };
        let mut pairing = Context::default();
        app.on_action(&mut pairing, action_id("quick-pair"));
        assert_eq!(app.route, Route::Pairing);
        assert!(app.seek_task.is_some());

        let started = api::parse_event(
            br#"{"type":"gameStart","game":{"id":"abcdEF12","color":"white","rated":true,"speed":"rapid","source":"lobby","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"Other"}}}"#,
        )
        .expect("gameStart");
        app.handle_event(&mut pairing, started);
        assert_eq!(app.route, Route::Game);
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("abcdEF12")
        );
        assert!(pairing
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Cancel(_))));

        let full = api::parse_board(
            br#"{"type":"gameFull","id":"abcdEF12","rated":true,"speed":"rapid","variant":{"key":"standard"},"initialFen":"startpos","white":{"id":"owner123","name":"Owner","rating":1500},"black":{"id":"other123","name":"Other","rating":1510},"state":{"type":"gameState","moves":"","wtime":600000,"btime":600000,"winc":0,"binc":0,"status":"started"}}"#,
            "abcdEF12",
        )
        .expect("gameFull");
        let mut stream = Context::default();
        app.handle_board(&mut stream, "abcdEF12", full);
        assert!(app.game.as_ref().expect("game").state.moves.is_empty());

        let mut move_context = Context::default();
        app.on_action(&mut move_context, action_id("square-e2"));
        app.on_action(&mut move_context, action_id("square-e4"));
        assert!(app.pending_move.is_some());
        app.handle_completed(
            &mut move_context,
            Pending::Action(GameAction::Move("e2e4".to_owned())),
            &[],
        );
        assert!(
            app.pending_move.is_some(),
            "POST success is not board acknowledgement"
        );

        let owner_move = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4","wtime":599000,"btime":600000,"winc":0,"binc":0,"status":"started"}"#,
            "abcdEF12",
        )
        .expect("owner state");
        app.handle_board(&mut stream, "abcdEF12", owner_move);
        assert!(app.pending_move.is_none());

        let opponent_move = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"started"}"#,
            "abcdEF12",
        )
        .expect("opponent state");
        app.handle_board(&mut stream, "abcdEF12", opponent_move);
        assert!(app.game.as_ref().expect("game").my_turn());

        let stale = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4","wtime":599000,"btime":599000,"winc":0,"binc":0,"status":"started"}"#,
            "abcdEF12",
        )
        .expect("stale state");
        app.handle_board(&mut stream, "abcdEF12", stale);
        assert!(app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("reopening authoritative"));

        let reconnected = api::parse_board(
            br#"{"type":"gameFull","id":"abcdEF12","rated":true,"speed":"rapid","variant":{"key":"standard"},"initialFen":"startpos","white":{"id":"owner123","name":"Owner","rating":1500},"black":{"id":"other123","name":"Other","rating":1510},"state":{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"started","bdraw":true}}"#,
            "abcdEF12",
        )
        .expect("reconnected full");
        app.handle_board(&mut stream, "abcdEF12", reconnected);
        assert!(app.game.as_ref().expect("game").draw_offer_from_opponent());

        let mut draw_context = Context::default();
        app.on_action(&mut draw_context, action_id("accept-draw"));
        assert_eq!(app.pending_action, Some(GameAction::AcceptDraw));
        app.handle_completed(
            &mut draw_context,
            Pending::Action(GameAction::AcceptDraw),
            &[],
        );
        let finished = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"draw"}"#,
            "abcdEF12",
        )
        .expect("draw finish");
        app.handle_board(&mut draw_context, "abcdEF12", finished);
        assert!(!app.game.as_ref().expect("finished game").active());
        assert!(app.session.is_none());
        assert_eq!(app.game.as_ref().expect("game").result(), "Draw agreed");
    }

    #[test]
    fn deterministic_mock_handles_challenge_resign_and_server_finish() {
        let mut app = app_with_game(&["e2e4"], Color::Black);
        app.event_open = true;
        let challenge = api::parse_event(
            br#"{"type":"challenge","challenge":{"id":"chall123","status":"created","direction":"in","challenger":{"name":"ReaderTwo"},"rated":false,"variant":{"key":"standard"},"speed":"rapid","timeControl":{"type":"clock","limit":600,"increment":0}}}"#,
        )
        .expect("challenge");
        let mut context = Context::default();
        app.handle_event(&mut context, challenge);
        assert_eq!(
            app.challenge
                .as_ref()
                .map(|challenge| challenge.id.as_str()),
            Some("chall123")
        );
        app.route = Route::Challenge;
        app.on_action(&mut context, action_id("accept-challenge"));
        assert!(matches!(
            app.pending_action,
            Some(GameAction::AcceptChallenge(_))
        ));

        app.pending_action = None;
        app.route = Route::Game;
        app.on_action(&mut context, action_id("confirm-resign"));
        app.on_action(&mut context, action_id("resign"));
        assert_eq!(app.pending_action, Some(GameAction::Resign));
        app.handle_completed(&mut context, Pending::Action(GameAction::Resign), &[]);
        let finished = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4","wtime":599000,"btime":600000,"winc":0,"binc":0,"status":"resign","winner":"white"}"#,
            "abcdEF12",
        )
        .expect("resign state");
        app.handle_board(&mut context, "abcdEF12", finished);
        assert_eq!(
            app.game.as_ref().expect("game").result(),
            "White won by resignation"
        );
        assert!(app.session.is_none());
    }

    #[test]
    fn clocks_are_large_stable_minute_second_strings() {
        assert_eq!(clock(600_000), "10:00");
        assert_eq!(clock(9_000), "00:09");
    }
}
