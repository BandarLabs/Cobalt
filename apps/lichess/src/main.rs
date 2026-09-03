//! An unofficial Lichess Board API client for Cobalt.
//!
//! The application names the `lichess` secret but never receives its value.
//! Live HTTP streams, redirects, cancellation, and credential attachment stay
//! in the runtime.

mod api;
mod chess;
mod model;

use api::{BoardRecord, Event, SeekPreset};
use kobo_json::{ObjectBuilder, Value};
use kobo_sdk::{
    action_id, ActionId, BandAlign, BannerLevel, Context, ControlState, Failure, Glyph, Heartbeat,
    KoboApp, Screen, ScreenBuilder, SlotWidth, StoreResult, Task, TaskError, TaskId, TaskOutcome,
    Tile, TileShape, TileState,
};
use model::{
    Account, ApplyState, Challenge, ChallengeDirection, Color, FullGame, Game, GameSummary, Session,
};
#[cfg(any(test, debug_assertions))]
use model::{ChallengeTime, Player, ServerState};
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::time::Duration;

const SESSION_KEY: &str = "lichess.session.v1";
const PUZZLE_KEY: &str = "lichess.puzzles.v1";
const BOARD_RATE_KEY: &str = "lichess.board-rate.v1";
const EVENT_RATE_KEY: &str = "lichess.event-rate.v1";
const SEEK_RATE_KEY: &str = "lichess.seek-rate.v1";
const MAX_STORED_PUZZLES: usize = 32;
const ACCOUNT_RETRY_SECONDS: u32 = 15;
const HOME_TILE_COUNT: usize = SeekPreset::ALL.len() + 2;

struct HomeTile {
    action: String,
    label: String,
    glyph: Glyph,
    subtitle: String,
    enabled: bool,
}

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
    AccountRetry,
    Playing,
    Puzzle,
    EventOpen,
    EventNext,
    EventRetry,
    EventRateWait {
        remaining: u32,
    },
    EventClose,
    Seek {
        generation: u64,
        preset: SeekPreset,
    },
    SeekGrace {
        generation: u64,
    },
    SeekReconcile {
        generation: u64,
    },
    SeekRateWait {
        remaining: u32,
    },
    BoardOpen(String),
    BoardNext(String),
    BoardRetry(String),
    BoardRateWait {
        id: String,
        remaining: u32,
    },
    BoardClose(String),
    Action {
        action: GameAction,
        scope: ActionScope,
        generation: u64,
    },
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActionScope {
    Game(String),
    Challenge(String),
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
    game_id: String,
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
        let fen = puzzle
            .get("fen")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| chess::puzzle_position(game.get("pgn")?.as_str()?, initial_ply))?;
        let first_move = solution.first()?;
        chess::legal(&fen, first_move).then_some(Self {
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
        let progress_is_valid = cursor == solution.len()
            || solution
                .get(cursor)
                .is_some_and(|movement| chess::legal(&fen, movement));
        (!solution.is_empty() && cursor <= solution.len() && progress_is_valid).then_some(Self {
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
    home_page: usize,
    account: AccountState,
    loaded_session: bool,
    loaded_puzzles: bool,
    loaded_board_rate: bool,
    loaded_event_rate: bool,
    loaded_seek_rate: bool,
    playing_ready: bool,
    session: Option<Session>,
    summaries: Vec<GameSummary>,
    game: Option<Game>,
    challenge: Option<Challenge>,
    tasks: BTreeMap<TaskId, Pending>,
    retired_tasks: BTreeMap<TaskId, Pending>,
    event_open: bool,
    deferred_event_open: bool,
    deferred_event_next: bool,
    deferred_event_close: bool,
    event_rate_limit: Option<u64>,
    board_open: Option<String>,
    board_ready: bool,
    deferred_board_open: Option<Session>,
    deferred_board_next: Option<String>,
    deferred_board_closes: BTreeSet<String>,
    board_rate_limits: BTreeMap<String, u64>,
    seek_task: Option<TaskId>,
    seek_waiting: bool,
    seek_generation: u64,
    selected_preset: Option<SeekPreset>,
    seek_baseline: BTreeSet<String>,
    seek_candidate: Option<GameSummary>,
    expected_seek_game: Option<(String, SeekPreset)>,
    seek_rate_limit: Option<u64>,
    pending_action: Option<GameAction>,
    pending_scope: Option<ActionScope>,
    pending_action_generation: Option<u64>,
    next_action_generation: u64,
    accepted_challenge: Option<Challenge>,
    reconcile_accepted_challenge: bool,
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
            home_page: 0,
            account: AccountState::Unknown,
            loaded_session: false,
            loaded_puzzles: false,
            loaded_board_rate: false,
            loaded_event_rate: false,
            loaded_seek_rate: false,
            playing_ready: false,
            session: None,
            summaries: Vec::new(),
            game: None,
            challenge: None,
            tasks: BTreeMap::new(),
            retired_tasks: BTreeMap::new(),
            event_open: false,
            deferred_event_open: false,
            deferred_event_next: false,
            deferred_event_close: false,
            event_rate_limit: None,
            board_open: None,
            board_ready: false,
            deferred_board_open: None,
            deferred_board_next: None,
            deferred_board_closes: BTreeSet::new(),
            board_rate_limits: BTreeMap::new(),
            seek_task: None,
            seek_waiting: false,
            seek_generation: 0,
            selected_preset: None,
            seek_baseline: BTreeSet::new(),
            seek_candidate: None,
            expected_seek_game: None,
            seek_rate_limit: None,
            pending_action: None,
            pending_scope: None,
            pending_action_generation: None,
            next_action_generation: 0,
            accepted_challenge: None,
            reconcile_accepted_challenge: false,
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
    fn show(&mut self, context: &mut Context) {
        let screen = match self.route {
            Route::Home => self.home(context),
            Route::Puzzles => self.puzzles_screen(),
            Route::Solve => self.solve_screen(),
            Route::PuzzleResult => self.puzzle_result(),
            Route::Play => self.play_screen(),
            Route::Pairing => self.pairing_screen(),
            Route::Challenge => self.challenge_screen(),
            Route::Game => self.game_screen(),
        };
        context.set_screen(screen.with_own_back(self.route != Route::Home));
    }

    fn seek_ready(&self) -> bool {
        matches!(self.account, AccountState::Ready(_))
            && !self.suspended
            && self.event_open
            && self.playing_ready
            && self.seek_task.is_none()
            && !self.seek_waiting
            && !self.has_pending(|pending| matches!(pending, Pending::SeekReconcile { .. }))
            && self.seek_rate_remaining().unwrap_or(0) == 0
            && self.game.as_ref().is_none_or(|game| !game.active())
            && self.accepted_challenge.is_none()
            && self.pending_action.is_none()
    }

    fn pairing_status(&self) -> String {
        if self.seek_ready() {
            return "Ready".to_owned();
        }
        if let Some(remaining) = self
            .seek_rate_remaining()
            .filter(|remaining| *remaining > 0)
        {
            return format!("Retry in {remaining}s");
        }
        match self.account {
            AccountState::Unknown | AccountState::Missing | AccountState::Invalid => {
                "Offline".to_owned()
            }
            AccountState::Checking => "Checking account".to_owned(),
            AccountState::Failed(_) => "Account error".to_owned(),
            AccountState::Ready(_) if !self.event_open => "Connecting".to_owned(),
            AccountState::Ready(_) if !self.playing_ready => "Refreshing games".to_owned(),
            AccountState::Ready(_) if self.game.as_ref().is_some_and(Game::active) => {
                "Board active".to_owned()
            }
            AccountState::Ready(_) => "Pairing in progress".to_owned(),
        }
    }

    fn home_header(&self) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("lichess-home")
            .top_bar("Lichess")
            .secondary(self.pairing_status());
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
    }

    fn home_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let measured = context.paginate_tiles_under(
            HOME_TILE_COUNT,
            TileShape::Square,
            false,
            &self.home_header().build(),
        );
        let columns = context
            .metrics()
            .grid_columns(TileShape::Square)
            .clamp(1, 3);
        let rows = if context.metrics().width > context.metrics().height {
            2
        } else {
            3
        };
        let capacity = measured
            .first()
            .map_or(columns, Vec::len)
            .min(columns * rows)
            .max(1);
        (0..HOME_TILE_COUNT)
            .collect::<Vec<_>>()
            .chunks(capacity)
            .map(<[usize]>::to_vec)
            .collect()
    }

    fn home_tile(index: usize, ready: bool) -> HomeTile {
        if index == 0 {
            HomeTile {
                action: "play".to_owned(),
                label: "Account/Games".to_owned(),
                glyph: Glyph::Settings,
                subtitle: "Challenges · boards".to_owned(),
                enabled: true,
            }
        } else if index == 1 {
            HomeTile {
                action: "puzzles".to_owned(),
                label: "Puzzles".to_owned(),
                glyph: Glyph::Grid,
                subtitle: "Offline training".to_owned(),
                enabled: true,
            }
        } else {
            let preset = SeekPreset::ALL[index - 2];
            HomeTile {
                action: preset.action().to_owned(),
                label: preset.label(),
                glyph: Glyph::Clock,
                subtitle: preset.speed_label().to_owned(),
                enabled: ready,
            }
        }
    }

    fn home_card_row(screen: ScreenBuilder, tiles: Vec<HomeTile>) -> ScreenBuilder {
        screen.band(
            BandAlign::Top,
            tiles.into_iter().map(|tile| {
                (
                    SlotWidth::Fill,
                    Box::new(move |slot: ScreenBuilder| {
                        let enabled = tile.enabled;
                        slot.tile_grid(
                            TileShape::Card,
                            [(tile.action, tile.label, tile.glyph, move |card: Tile| {
                                let card = card.with_subtitle(tile.subtitle);
                                if enabled {
                                    card
                                } else {
                                    card.with_state(TileState::Unavailable)
                                }
                            })],
                        )
                    }) as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                )
            }),
        )
    }

    fn home(&mut self, context: &Context) -> Screen {
        let pages = self.home_pages(context);
        self.home_page = self.home_page.min(pages.len().saturating_sub(1));
        let page = self.home_page;
        let page_count = u16::try_from(pages.len()).unwrap_or(u16::MAX);
        let page_index = u16::try_from(page).unwrap_or(u16::MAX);
        let page_number = u16::try_from(page.saturating_add(1)).unwrap_or(u16::MAX);
        let ready = self.seek_ready();
        let columns = context
            .metrics()
            .grid_columns(TileShape::Square)
            .clamp(1, 3);
        let mut screen = self.home_header().page_rail(page_index, page_count);
        for row in pages[page].chunks(columns) {
            let tiles = row
                .iter()
                .map(|index| Self::home_tile(*index, ready))
                .collect();
            screen = Self::home_card_row(screen, tiles);
        }
        screen
            .page_turns("home-previous", "home-next")
            .page_position(page_number, page_count)
            .build()
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
        let mut screen = ScreenBuilder::new("lichess-play").top_bar("Account/Games");
        screen = match &self.account {
            AccountState::Unknown | AccountState::Missing => screen.secondary("Offline"),
            AccountState::Checking => screen.activity("Checking account", None),
            AccountState::Ready(account) => screen.secondary(account.username.clone()),
            AccountState::Invalid => screen.banner(BannerLevel::Attention, "Token rejected"),
            AccountState::Failed(message) => screen.banner(BannerLevel::Attention, message),
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
        if let Some(game) = self.game.as_ref().filter(|game| {
            game.active()
                && self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.game_id == game.id)
        }) {
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
        screen
            .button_with_state(
                "refresh-play",
                "Refresh",
                if self
                    .has_pending(|pending| matches!(pending, Pending::Account | Pending::Playing))
                {
                    ControlState::Disabled
                } else {
                    ControlState::Enabled
                },
            )
            .build()
    }

    fn pairing_screen(&self) -> Screen {
        let preset = self.selected_preset.unwrap_or(SeekPreset::Rapid10_0);
        let clock = preset.label();
        if let Some(candidate) = &self.seek_candidate {
            return ScreenBuilder::new("lichess-pairing-candidate")
                .top_bar("Game started")
                .heading(format!("vs {}", candidate.opponent))
                .secondary(format!("{clock} {} · Rated", preset.speed_label()))
                .primary_button("open-seek-candidate", "Open game")
                .buttons([("keep-seeking", "Keep waiting"), ("cancel-seek", "Cancel")])
                .build();
        }
        let reconciling =
            self.has_pending(|pending| matches!(pending, Pending::SeekReconcile { .. }));
        let mut screen = ScreenBuilder::new("lichess-pairing")
            .top_bar("Pairing")
            .heading(format!("{clock} {} · Rated", preset.speed_label()))
            .activity(if reconciling { "Checking" } else { "Waiting" }, None)
            .secondary(self.clock.waited_words());
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
        let pending = self.pending_action.is_some() || self.accepted_challenge.is_some();
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
        let live = self.board_is_live(&game.id);
        let mut menu = Vec::new();
        if live {
            menu.push(("confirm-resign".to_owned(), "Resign".to_owned()));
            if game.can_abort() {
                menu.push(("confirm-abort".to_owned(), "Abort".to_owned()));
            }
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
                if !live {
                    "Board stream paused"
                } else if game.my_turn() {
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
        let blocked = !live || self.pending_action.is_some() || self.pending_move.is_some();
        if !live {
            let reconnecting = self.has_pending(|pending| {
                matches!(
                    pending,
                    Pending::BoardOpen(_)
                        | Pending::BoardNext(_)
                        | Pending::BoardRetry(_)
                        | Pending::BoardRateWait { .. }
                        | Pending::BoardClose(_)
                )
            });
            screen = screen.button_with_state(
                "reconnect-board",
                "Reconnect board",
                enabled(
                    !reconnecting && self.pending_action.is_none() && self.pending_move.is_none(),
                ),
            );
        } else if game.draw_offer_from_opponent() {
            screen = screen
                .button_with_state("accept-draw", "Accept draw", enabled(!blocked))
                .button_with_state("decline-draw", "Decline draw", enabled(!blocked));
        } else {
            screen = screen.button_with_state("offer-draw", "Offer draw", enabled(!blocked));
        }
        if live && game.opponent_gone && self.claim_remaining() == Some(0) {
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
        for (task, pending) in self.tasks.clone() {
            if matches!(pending, Pending::AccountRetry) {
                context.cancel(task);
                self.tasks.remove(&task);
                self.retired_tasks.insert(task, pending);
            }
        }
        self.account = AccountState::Checking;
        self.playing_ready = false;
        self.notice = None;
        if self
            .spawn(context, Pending::Account, api::account(), true)
            .is_none()
        {
            self.account = AccountState::Unknown;
            self.notice =
                Some("Finishing previous requests. Refresh account again in a moment.".to_owned());
        }
    }

    fn schedule_account_retry(&mut self, context: &mut Context) {
        if self.suspended
            || self
                .has_pending(|pending| matches!(pending, Pending::Account | Pending::AccountRetry))
        {
            return;
        }
        let _ = self.spawn(
            context,
            Pending::AccountRetry,
            Task::Sleep {
                seconds: ACCOUNT_RETRY_SECONDS,
            },
            false,
        );
    }

    fn refresh_playing(&mut self, context: &mut Context) -> bool {
        if self.suspended {
            return false;
        }
        if self.has_pending(|pending| {
            matches!(pending, Pending::Playing | Pending::SeekReconcile { .. })
        }) {
            return true;
        }
        self.spawn(context, Pending::Playing, api::playing(), true)
            .is_some()
    }

    fn set_event_rate_limit(&mut self, context: &mut Context, seconds: u32) {
        let not_before = unix_seconds().saturating_add(u64::from(seconds.max(1)));
        self.event_rate_limit = Some(not_before);
        context.store().save(
            EVENT_RATE_KEY,
            ObjectBuilder::new()
                .set("version", 1_u32)
                .set("not_before", u32::try_from(not_before).unwrap_or(u32::MAX))
                .build()
                .to_json()
                .into_bytes(),
        );
    }

    fn clear_event_rate_limit(&mut self, context: &mut Context) {
        if self.event_rate_limit.take().is_some() {
            context.store().forget(EVENT_RATE_KEY);
        }
    }

    fn event_rate_remaining(&self) -> Option<u32> {
        Some(
            u32::try_from(self.event_rate_limit?.saturating_sub(unix_seconds()))
                .unwrap_or(u32::MAX),
        )
    }

    fn schedule_event_rate_wait(&mut self, context: &mut Context, remaining: u32) {
        if self.suspended {
            self.deferred_event_open = true;
            return;
        }
        if self.has_pending(|pending| matches!(pending, Pending::EventRateWait { .. })) {
            self.deferred_event_open = false;
            return;
        }
        if remaining == 0 {
            self.clear_event_rate_limit(context);
            self.open_event_stream(context);
            return;
        }
        let step = remaining.min(300);
        self.deferred_event_open = self
            .spawn(
                context,
                Pending::EventRateWait {
                    remaining: remaining.saturating_sub(step),
                },
                Task::Sleep { seconds: step },
                false,
            )
            .is_none();
    }

    fn open_event_stream(&mut self, context: &mut Context) {
        if self.suspended {
            self.deferred_event_open = true;
            return;
        }
        if self.event_open || self.has_pending(|pending| matches!(pending, Pending::EventOpen)) {
            self.deferred_event_open = false;
            return;
        }
        if self.has_pending(|pending| matches!(pending, Pending::EventClose))
            || self.deferred_event_close
            || self
                .retired_tasks
                .values()
                .any(|pending| matches!(pending, Pending::EventClose))
            || !matches!(self.account, AccountState::Ready(_))
        {
            self.deferred_event_open = true;
            return;
        }
        if let Some(remaining) = self.event_rate_remaining() {
            if remaining > 0 {
                self.schedule_event_rate_wait(context, remaining);
                return;
            }
            self.clear_event_rate_limit(context);
        }
        self.deferred_event_open = self
            .spawn(
                context,
                Pending::EventOpen,
                api::event_stream("open"),
                false,
            )
            .is_none();
    }

    fn retry_deferred_event(&mut self, context: &mut Context) {
        if self.deferred_event_next {
            self.next_event(context);
        }
        if self.deferred_event_open {
            self.open_event_stream(context);
        }
    }

    fn next_event(&mut self, context: &mut Context) {
        if self.suspended || !self.event_open {
            self.deferred_event_next = self.event_open;
            return;
        }
        if self.has_pending(|pending| matches!(pending, Pending::EventNext)) {
            self.deferred_event_next = false;
            return;
        }
        self.deferred_event_next = self
            .spawn(
                context,
                Pending::EventNext,
                api::event_stream("next"),
                false,
            )
            .is_none();
    }

    fn flush_deferred_stream_closes(&mut self, context: &mut Context) {
        let event_close_pending = self
            .has_pending(|pending| matches!(pending, Pending::EventClose))
            || self
                .retired_tasks
                .values()
                .any(|pending| matches!(pending, Pending::EventClose));
        if self.deferred_event_close {
            let started = event_close_pending
                || self
                    .spawn(
                        context,
                        Pending::EventClose,
                        api::event_stream("close"),
                        false,
                    )
                    .is_some();
            if started {
                self.deferred_event_close = false;
            }
        }
        for id in self.deferred_board_closes.clone() {
            if self.has_pending(
                |pending| matches!(pending, Pending::BoardClose(closing) if closing == &id),
            ) || self
                .retired_tasks
                .values()
                .any(|pending| matches!(pending, Pending::BoardClose(closing) if closing == &id))
            {
                self.deferred_board_closes.remove(&id);
                continue;
            }
            let Some(work) = api::board_stream(&id, "close") else {
                self.deferred_board_closes.remove(&id);
                continue;
            };
            if self
                .spawn(context, Pending::BoardClose(id.clone()), work, false)
                .is_some()
            {
                self.deferred_board_closes.remove(&id);
            } else {
                break;
            }
        }
    }

    fn close_event(&mut self, context: &mut Context) {
        self.event_open = false;
        self.deferred_event_next = false;
        self.deferred_event_close = true;
        self.flush_deferred_stream_closes(context);
    }

    fn schedule_event_reconnect(&mut self, context: &mut Context) {
        self.event_open = false;
        self.deferred_event_next = false;
        if self.suspended {
            self.deferred_event_open = true;
            return;
        }
        if self.has_pending(|pending| matches!(pending, Pending::EventOpen | Pending::EventRetry))
            || !matches!(self.account, AccountState::Ready(_))
        {
            self.deferred_event_open = true;
            return;
        }
        let seconds = self.event_backoff.min(30);
        self.event_backoff = self.event_backoff.saturating_mul(2).min(30);
        self.deferred_event_open = self
            .spawn(context, Pending::EventRetry, Task::Sleep { seconds }, false)
            .is_none();
    }

    #[allow(
        clippy::too_many_lines,
        reason = "board opening serializes switching, persisted backoff, close fencing, and task-capacity deferral"
    )]
    fn open_board(&mut self, context: &mut Context, session: Session) {
        if self.suspended {
            return;
        }
        let id = session.game_id.clone();
        if self
            .expected_seek_game
            .as_ref()
            .is_some_and(|(expected, _)| expected != &id)
        {
            self.expected_seek_game = None;
        }
        if self.accepted_challenge.is_some() {
            self.notice = Some(
                "Wait for the accepted challenge to start before opening another board.".to_owned(),
            );
            return;
        }
        if (self.pending_action.is_some() || self.pending_move.is_some())
            && self.game.as_ref().is_some_and(|game| game.id != id)
        {
            self.notice =
                Some("Wait for the current game action before switching boards.".to_owned());
            return;
        }
        self.route = Route::Game;
        let previous = self
            .session
            .as_ref()
            .map(|session| session.game_id.clone())
            .or_else(|| self.game.as_ref().map(|game| game.id.clone()))
            .filter(|previous| previous != &id);
        if let Some(previous) = previous {
            for (task, pending) in self.tasks.clone() {
                if matches!(
                    &pending,
                    Pending::BoardOpen(open)
                        | Pending::BoardNext(open)
                        | Pending::BoardRetry(open)
                        if open == &previous
                ) || matches!(
                    &pending,
                    Pending::BoardRateWait { id: waiting, .. } if waiting == &previous
                ) {
                    context.cancel(task);
                    self.tasks.remove(&task);
                    self.retired_tasks.insert(task, pending);
                }
            }
            self.deferred_board_closes.insert(previous);
            self.flush_deferred_stream_closes(context);
            self.board_open = None;
            self.board_ready = false;
            self.deferred_board_next = None;
            self.game = None;
            self.selected = None;
            self.promotion = None;
            self.confirmation = None;
            self.menu_open = false;
            self.clock.stop(context);
        }
        self.session = Some(session);
        self.persist_session(context);
        if let Some(remaining) = self.board_rate_remaining(&id) {
            if remaining > 0 {
                self.board_open = None;
                self.board_ready = false;
                self.notice = Some(format!(
                    "Lichess asked this board to wait {remaining}s before reconnecting."
                ));
                self.deferred_board_open = None;
                self.schedule_board_rate_wait(context, &id, remaining);
                return;
            }
            self.clear_board_rate_limit(context, &id);
        }
        if self.has_pending(
            |pending| matches!(pending, Pending::BoardClose(closing) if closing == &id),
        ) || self
            .retired_tasks
            .values()
            .any(|pending| matches!(pending, Pending::BoardClose(closing) if closing == &id))
            || self.deferred_board_closes.contains(&id)
        {
            self.deferred_board_open = self.session.clone();
            return;
        }
        if self.board_open.as_deref() == Some(id.as_str())
            || self.has_pending(|pending| {
                matches!(
                    pending,
                    Pending::BoardOpen(open)
                        | Pending::BoardRetry(open)
                        if open == &id
                ) || matches!(
                    pending,
                    Pending::BoardRateWait { id: waiting, .. } if waiting == &id
                )
            })
        {
            self.deferred_board_open = None;
            return;
        }
        let Some(work) = api::board_stream(&id, "open") else {
            self.notice = Some("Lichess returned an invalid game identifier.".to_owned());
            return;
        };
        self.board_ready = false;
        if self
            .spawn(context, Pending::BoardOpen(id), work, false)
            .is_some()
        {
            self.deferred_board_open = None;
            Self::keep_live(context);
        } else {
            self.deferred_board_open = self.session.clone();
            self.notice = Some("Waiting for a task slot before reopening the board.".to_owned());
        }
    }

    fn next_board(&mut self, context: &mut Context, id: &str) {
        if self.suspended || self.board_open.as_deref() != Some(id) {
            if self.board_open.as_deref() == Some(id) {
                self.deferred_board_next = Some(id.to_owned());
            }
            return;
        }
        if self.has_pending(|pending| matches!(pending, Pending::BoardNext(open) if open == id)) {
            self.deferred_board_next = None;
            return;
        }
        let Some(work) = api::board_stream(id, "next") else {
            return;
        };
        if self
            .spawn(context, Pending::BoardNext(id.to_owned()), work, false)
            .is_some()
        {
            self.deferred_board_next = None;
        } else {
            self.deferred_board_next = Some(id.to_owned());
        }
    }

    fn close_board(&mut self, context: &mut Context, id: &str) {
        if self.board_open.as_deref() == Some(id) {
            self.board_open = None;
            self.board_ready = false;
            self.deferred_board_next = None;
        }
        self.deferred_board_closes.insert(id.to_owned());
        self.flush_deferred_stream_closes(context);
    }

    fn discard_game(&mut self, context: &mut Context, game_id: &str) {
        for (task, pending) in self.tasks.clone() {
            if matches!(
                &pending,
                Pending::BoardOpen(game)
                    | Pending::BoardNext(game)
                    | Pending::BoardRetry(game)
                    if game == game_id
            ) || matches!(
                &pending,
                Pending::BoardRateWait { id, .. } if id == game_id
            ) {
                context.cancel(task);
                self.tasks.remove(&task);
                self.retired_tasks.insert(task, pending);
            }
        }
        self.close_board(context, game_id);
        self.clock.stop(context);
        self.game = None;
        self.clear_pending_action();
        self.accepted_challenge = None;
        self.reconcile_accepted_challenge = false;
        self.pending_move = None;
        self.expected_seek_game = None;
        self.selected = None;
        self.promotion = None;
        self.confirmation = None;
        self.menu_open = false;
        self.board_ready = false;
        self.deferred_board_next = None;
        if self
            .deferred_board_open
            .as_ref()
            .is_some_and(|session| session.game_id == game_id)
        {
            self.deferred_board_open = None;
        }
        self.clear_board_rate_limit(context, game_id);
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.game_id == game_id)
        {
            self.clear_session(context);
        }
        if self.route == Route::Game {
            self.route = Route::Play;
        }
    }

    fn schedule_board_reconnect(&mut self, context: &mut Context, id: &str) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.game_id != id)
        {
            return;
        }
        self.board_open = None;
        self.board_ready = false;
        self.deferred_board_next = None;
        self.clock.stop(context);
        if self.suspended
            || self.has_pending(|pending| {
                matches!(pending, Pending::BoardOpen(open) | Pending::BoardRetry(open) if open == id)
            })
        {
            return;
        }
        let seconds = self.board_backoff.min(30);
        self.board_backoff = self.board_backoff.saturating_mul(2).min(30);
        if self
            .spawn(
                context,
                Pending::BoardRetry(id.to_owned()),
                Task::Sleep { seconds },
                false,
            )
            .is_none()
        {
            self.deferred_board_open = self.session.clone();
        }
    }

    fn schedule_board_rate_wait(&mut self, context: &mut Context, id: &str, remaining: u32) {
        if self.suspended {
            return;
        }
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.game_id != id)
        {
            return;
        }
        if self.has_pending(|pending| {
            matches!(
                pending,
                Pending::BoardRateWait { id: waiting, .. } if waiting == id
            )
        }) {
            return;
        }
        if remaining == 0 {
            self.clear_board_rate_limit(context, id);
            if let Some(session) = self.session.clone().filter(|session| session.game_id == id) {
                self.open_board(context, session);
            }
            return;
        }
        let step = remaining.min(300);
        if self
            .spawn(
                context,
                Pending::BoardRateWait {
                    id: id.to_owned(),
                    remaining: remaining.saturating_sub(step),
                },
                Task::Sleep { seconds: step },
                false,
            )
            .is_none()
        {
            self.deferred_board_open = self.session.clone();
        }
    }

    fn board_is_live(&self, id: &str) -> bool {
        self.board_ready && self.board_open.as_deref() == Some(id)
    }

    fn retry_deferred_board(&mut self, context: &mut Context) {
        if self.suspended {
            return;
        }
        if let Some(id) = self.deferred_board_next.clone() {
            self.next_board(context, &id);
            if self.deferred_board_next.is_some() {
                return;
            }
        }
        if let Some(session) = self.deferred_board_open.clone() {
            self.open_board(context, session);
        }
    }

    fn set_seek_rate_limit(&mut self, context: &mut Context, seconds: u32) {
        let not_before = unix_seconds().saturating_add(u64::from(seconds.max(1)));
        self.seek_rate_limit = Some(not_before);
        context.store().save(
            SEEK_RATE_KEY,
            ObjectBuilder::new()
                .set("version", 1_u32)
                .set("not_before", u32::try_from(not_before).unwrap_or(u32::MAX))
                .build()
                .to_json()
                .into_bytes(),
        );
    }

    fn clear_seek_rate_limit(&mut self, context: &mut Context) {
        if self.seek_rate_limit.take().is_some() {
            context.store().forget(SEEK_RATE_KEY);
        }
    }

    fn seek_rate_remaining(&self) -> Option<u32> {
        Some(
            u32::try_from(self.seek_rate_limit?.saturating_sub(unix_seconds())).unwrap_or(u32::MAX),
        )
    }

    fn schedule_seek_rate_wait(&mut self, context: &mut Context, remaining: u32) {
        if self.suspended
            || self.has_pending(|pending| matches!(pending, Pending::SeekRateWait { .. }))
        {
            return;
        }
        if remaining == 0 {
            self.clear_seek_rate_limit(context);
            return;
        }
        let step = remaining.min(300);
        let _ = self.spawn(
            context,
            Pending::SeekRateWait {
                remaining: remaining.saturating_sub(step),
            },
            Task::Sleep { seconds: step },
            false,
        );
    }

    fn seek_is_current(&self, generation: u64, preset: SeekPreset) -> bool {
        self.seek_waiting
            && self.seek_generation == generation
            && self.selected_preset == Some(preset)
    }

    fn start_seek(&mut self, context: &mut Context, preset: SeekPreset) {
        if let Some(remaining) = self.seek_rate_remaining() {
            if remaining > 0 {
                self.notice = Some(format!(
                    "Lichess asked pairing to wait {remaining}s before another seek."
                ));
                self.schedule_seek_rate_wait(context, remaining);
                return;
            }
            self.clear_seek_rate_limit(context);
        }
        if !self.seek_ready() {
            self.notice = Some("Offline.".to_owned());
            return;
        }
        self.seek_generation = self.seek_generation.wrapping_add(1);
        let generation = self.seek_generation;
        self.seek_baseline = self
            .summaries
            .iter()
            .map(|game| game.id.clone())
            .chain(self.game.iter().map(|game| game.id.clone()))
            .collect();
        self.seek_waiting = true;
        self.selected_preset = Some(preset);
        self.seek_candidate = None;
        if let Some(task) = self.spawn(
            context,
            Pending::Seek { generation, preset },
            api::seek(preset),
            false,
        ) {
            self.seek_task = Some(task);
            self.route = Route::Pairing;
            self.notice = None;
            self.reset_clock(context, true);
            Self::keep_live(context);
        } else {
            self.seek_waiting = false;
            self.selected_preset = None;
            self.seek_baseline.clear();
        }
    }

    fn cancel_seek(&mut self, context: &mut Context) {
        let generation = self.seek_generation;
        self.seek_waiting = false;
        self.selected_preset = None;
        self.seek_baseline.clear();
        self.seek_candidate = None;
        for (task, pending) in self.tasks.clone() {
            if matches!(
                pending,
                Pending::SeekGrace {
                    generation: pending_generation
                } | Pending::SeekReconcile {
                    generation: pending_generation
                } if pending_generation == generation
            ) {
                context.cancel(task);
                self.tasks.remove(&task);
            }
        }
        if let Some(task) = self.seek_task {
            context.cancel(task);
            self.notice = Some("Cancelling the pending seek.".to_owned());
        } else if self.route == Route::Pairing {
            self.clock.stop(context);
            self.route = Route::Play;
            self.notice = Some("Pairing cancelled. No duplicate seek was created.".to_owned());
        }
    }

    fn open_seek_candidate(&mut self, context: &mut Context) {
        let Some(candidate) = self.seek_candidate.take() else {
            return;
        };
        let Some(preset) = self.selected_preset.take() else {
            return;
        };
        self.seek_waiting = false;
        self.seek_baseline.clear();
        self.expected_seek_game = Some((candidate.id.clone(), preset));
        if let Some(task) = self.seek_task.take() {
            context.cancel(task);
        }
        for (task, pending) in self.tasks.clone() {
            if matches!(
                pending,
                Pending::SeekGrace { .. } | Pending::SeekReconcile { .. }
            ) {
                context.cancel(task);
                self.tasks.remove(&task);
            }
        }
        self.notice = Some("Opening the confirmed game without replaying the seek.".to_owned());
        self.open_board(context, candidate.session());
    }

    fn await_seek_event(
        &mut self,
        context: &mut Context,
        generation: u64,
        preset: SeekPreset,
        message: String,
    ) {
        if !self.seek_is_current(generation, preset) {
            return;
        }
        self.seek_task = None;
        self.notice = Some(message);
        if !self.has_pending(|pending| {
            matches!(
                pending,
                Pending::SeekGrace {
                    generation: pending_generation
                } if *pending_generation == generation
            )
        }) {
            let _ = self.spawn(
                context,
                Pending::SeekGrace { generation },
                Task::Sleep { seconds: 10 },
                false,
            );
        }
    }

    fn reconcile_ended_seek(&mut self, context: &mut Context, generation: u64) {
        if !self.seek_waiting
            || self.seek_generation != generation
            || self.selected_preset.is_none()
            || self.seek_candidate.is_some()
        {
            return;
        }
        self.notice = Some("Checking games.".to_owned());
        if self
            .spawn(
                context,
                Pending::SeekReconcile { generation },
                api::playing(),
                true,
            )
            .is_none()
        {
            let _ = self.spawn(
                context,
                Pending::SeekGrace { generation },
                Task::Sleep { seconds: 1 },
                false,
            );
        }
    }

    fn finish_seek_reconciliation(
        &mut self,
        context: &mut Context,
        generation: u64,
        games: Vec<GameSummary>,
    ) {
        if !self.seek_waiting || self.seek_generation != generation {
            return;
        }
        let Some(preset) = self.selected_preset else {
            return;
        };
        self.playing_ready = true;
        self.summaries = games;
        if self.seek_candidate.is_none() {
            let mut candidates = self.summaries.iter().filter(|summary| {
                !self.seek_baseline.contains(&summary.id) && preset.matches_summary(summary)
            });
            let first = candidates.next().cloned();
            let ambiguous = candidates.next().is_some();
            if !ambiguous {
                self.seek_candidate = first;
            }
            if ambiguous {
                self.seek_waiting = false;
                self.selected_preset = None;
                self.seek_baseline.clear();
                self.clock.stop(context);
                self.route = Route::Play;
                self.notice = Some(
                    "Several new games matched the selected preset. Choose the intended game from Ongoing games."
                        .to_owned(),
                );
                return;
            }
        }
        if let Some(candidate) = &self.seek_candidate {
            self.notice = Some(format!(
                "Recovered a new rated {} {} game from current games. Confirm {} before opening.",
                preset.label(),
                preset.speed_label(),
                candidate.opponent
            ));
        } else {
            self.seek_waiting = false;
            self.selected_preset = None;
            self.seek_baseline.clear();
            self.clock.stop(context);
            self.route = Route::Play;
            self.notice = Some("No match.".to_owned());
        }
    }

    fn retry_seek_reconciliation(
        &mut self,
        context: &mut Context,
        generation: u64,
        message: String,
        seconds: u32,
    ) {
        if !self.seek_waiting || self.seek_generation != generation {
            return;
        }
        self.notice = Some(message);
        let _ = self.spawn(
            context,
            Pending::SeekGrace { generation },
            Task::Sleep {
                seconds: seconds.max(1),
            },
            false,
        );
    }

    fn send_action(&mut self, context: &mut Context, action: GameAction, work: Option<Task>) {
        if self.pending_action.is_some() || work.is_none() {
            return;
        }
        let scope = match &action {
            GameAction::AcceptChallenge(id) | GameAction::DeclineChallenge(id) => {
                ActionScope::Challenge(id.clone())
            }
            _ => {
                let Some(game) = &self.game else {
                    return;
                };
                if !self.board_is_live(&game.id) {
                    self.notice = Some("Reconnect the board before sending an action.".to_owned());
                    return;
                }
                ActionScope::Game(game.id.clone())
            }
        };
        if let GameAction::Move(movement) = &action {
            let game = self.game.as_ref().expect("game-scoped move");
            self.pending_move = Some(PendingMove {
                game_id: game.id.clone(),
                movement: movement.clone(),
                at_ply: game.state.moves.len(),
            });
        }
        if let GameAction::AcceptChallenge(id) = &action {
            self.accepted_challenge = self
                .challenge
                .as_ref()
                .filter(|challenge| challenge.id == id.as_str())
                .cloned();
            self.reconcile_accepted_challenge = false;
        }
        self.next_action_generation = self.next_action_generation.saturating_add(1);
        let generation = self.next_action_generation;
        self.pending_action = Some(action.clone());
        self.pending_scope = Some(scope.clone());
        self.pending_action_generation = Some(generation);
        if self
            .spawn(
                context,
                Pending::Action {
                    action,
                    scope,
                    generation,
                },
                work.expect("checked"),
                false,
            )
            .is_none()
        {
            self.clear_pending_action();
            self.accepted_challenge = None;
            self.reconcile_accepted_challenge = false;
            self.pending_move = None;
            self.notice = Some("Too many requests are already in flight.".to_owned());
        }
    }

    fn action_scope_is_current(&self, scope: &ActionScope) -> bool {
        match scope {
            ActionScope::Game(id) => self
                .game
                .as_ref()
                .is_some_and(|game| game.id == id.as_str()),
            ActionScope::Challenge(id) => self
                .challenge
                .as_ref()
                .is_some_and(|challenge| challenge.id == id.as_str()),
        }
    }

    fn pending_action_is(&self, action: &GameAction, scope: &ActionScope, generation: u64) -> bool {
        self.pending_action.as_ref() == Some(action)
            && self.pending_scope.as_ref() == Some(scope)
            && self.pending_action_generation == Some(generation)
    }

    fn clear_pending_action(&mut self) {
        self.pending_action = None;
        self.pending_scope = None;
        self.pending_action_generation = None;
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
        if !self.board_is_live(&game.id) {
            self.notice = Some("Reconnect the board before making a move.".to_owned());
            return;
        }
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
        if puzzle.cursor >= puzzle.solution.len() {
            self.route = Route::PuzzleResult;
            return;
        }
        let solver = chess::side_to_move(&puzzle.fen);
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
        let mut reply = None;
        while puzzle.cursor < puzzle.solution.len() && chess::side_to_move(&puzzle.fen) != solver {
            let forced = puzzle.solution[puzzle.cursor].clone();
            let Some((fen, san)) = chess::play(&puzzle.fen, &forced) else {
                self.puzzle_wrong = 2;
                self.notice = Some("The stored puzzle reply could not be replayed.".to_owned());
                self.route = Route::PuzzleResult;
                return;
            };
            puzzle.fen = fen;
            puzzle.cursor = puzzle.cursor.saturating_add(1);
            reply = Some(san);
        }
        if puzzle.cursor >= puzzle.solution.len() {
            self.route = Route::PuzzleResult;
        } else {
            self.notice = reply.map(|reply| format!("Opponent replied {reply}."));
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
        ObjectBuilder::new()
            .set("version", 2_u32)
            .set(
                "current",
                u32::try_from(self.current_puzzle).unwrap_or(u32::MAX),
            )
            .set("wrong", u32::from(self.puzzle_wrong))
            .set(
                "puzzles",
                Value::Array(self.puzzles.iter().map(Puzzle::stored).collect()),
            )
            .build()
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
        if value.get("version").and_then(Value::as_i64) != Some(2) {
            return false;
        }
        let Some(items) = value.get("puzzles").and_then(Value::as_array) else {
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
        let Some(current) = value
            .get("current")
            .and_then(Value::as_i64)
            .and_then(|current| usize::try_from(current).ok())
        else {
            return false;
        };
        if current > puzzles.len() {
            return false;
        }
        let Some(wrong) = value
            .get("wrong")
            .and_then(Value::as_i64)
            .and_then(|wrong| u8::try_from(wrong).ok())
            .filter(|wrong| *wrong <= 2)
        else {
            return false;
        };
        self.puzzles = puzzles;
        self.current_puzzle = current;
        self.puzzle_wrong = wrong;
        while self
            .puzzles
            .get(self.current_puzzle)
            .is_some_and(|puzzle| puzzle.cursor >= puzzle.solution.len())
        {
            self.current_puzzle = self.current_puzzle.saturating_add(1);
            self.puzzle_wrong = 0;
        }
        true
    }

    fn persist_session(&self, context: &mut Context) {
        if let Some(session) = &self.session {
            context.store().save(SESSION_KEY, session.encode());
        }
    }

    fn set_board_rate_limit(&mut self, context: &mut Context, id: &str, seconds: u32) {
        let now = unix_seconds();
        self.board_rate_limits
            .retain(|_, not_before| *not_before > now);
        if self.board_rate_limits.len() >= 50 && !self.board_rate_limits.contains_key(id) {
            if let Some(oldest) = self
                .board_rate_limits
                .iter()
                .min_by_key(|(_, not_before)| *not_before)
                .map(|(id, _)| id.clone())
            {
                self.board_rate_limits.remove(&oldest);
            }
        }
        let not_before = unix_seconds().saturating_add(u64::from(seconds.max(1)));
        self.board_rate_limits.insert(id.to_owned(), not_before);
        self.persist_board_rate_limits(context);
    }

    fn clear_board_rate_limit(&mut self, context: &mut Context, id: &str) {
        if self.board_rate_limits.remove(id).is_some() {
            self.persist_board_rate_limits(context);
        }
    }

    fn board_rate_remaining(&self, id: &str) -> Option<u32> {
        let not_before = self.board_rate_limits.get(id)?;
        Some(u32::try_from(not_before.saturating_sub(unix_seconds())).unwrap_or(u32::MAX))
    }

    fn persist_board_rate_limits(&self, context: &mut Context) {
        if self.board_rate_limits.is_empty() {
            context.store().forget(BOARD_RATE_KEY);
            return;
        }
        let limits = self
            .board_rate_limits
            .iter()
            .map(|(id, not_before)| {
                ObjectBuilder::new()
                    .set("game_id", id.as_str())
                    .set("not_before", u32::try_from(*not_before).unwrap_or(u32::MAX))
                    .build()
            })
            .collect();
        context.store().save(
            BOARD_RATE_KEY,
            ObjectBuilder::new()
                .set("version", 2_u32)
                .set("limits", Value::Array(limits))
                .build()
                .to_json()
                .into_bytes(),
        );
    }

    fn clear_session(&mut self, context: &mut Context) {
        self.session = None;
        context.store().forget(SESSION_KEY);
    }

    fn selected_board_matches(&mut self, context: &mut Context, id: &str, full: &FullGame) -> bool {
        let Some((expected_id, preset)) = self.expected_seek_game.clone() else {
            return true;
        };
        if expected_id != id {
            return true;
        }
        self.expected_seek_game = None;
        if preset.matches_full(full) {
            return true;
        }
        self.notice = Some(format!(
            "The opened game was not the selected {} {} clock. It remains available in Ongoing games.",
            preset.label(),
            preset.speed_label()
        ));
        self.close_board(context, id);
        self.clear_session(context);
        self.game = None;
        self.route = Route::Play;
        false
    }

    fn maybe_start(&mut self, context: &mut Context) {
        if self.loaded_session
            && self.loaded_puzzles
            && self.loaded_board_rate
            && self.loaded_event_rate
            && self.loaded_seek_rate
        {
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
        self.clear_pending_action();
        self.accepted_challenge = None;
        self.reconcile_accepted_challenge = false;
        self.pending_move = None;
        self.expected_seek_game = None;
        self.selected = None;
        self.promotion = None;
        self.confirmation = None;
        if matches!(self.route, Route::Pairing | Route::Challenge | Route::Game) {
            self.route = Route::Play;
        }
        self.close_live_reads(context);
        self.schedule_account_retry(context);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the account stream has five event kinds and two explicitly correlated game-start paths"
    )]
    fn handle_event(&mut self, context: &mut Context, event: Event) {
        match event {
            Event::GameStart(summary) => {
                if !summary.supported() {
                    if self.route == Route::Play {
                        self.notice = Some(
                            "A non-standard game started and was not offered here.".to_owned(),
                        );
                    }
                    return;
                }
                let seek_match = self.seek_waiting
                    && self
                        .selected_preset
                        .is_some_and(|preset| preset.matches_summary(&summary))
                    && !self.seek_baseline.contains(&summary.id);
                let accepted_challenge = self
                    .accepted_challenge
                    .as_ref()
                    .filter(|challenge| challenge.matches_game_start(&summary))
                    .map(|challenge| challenge.id.clone());
                self.upsert_summary(summary.clone());
                if let Some(challenge_id) = accepted_challenge {
                    if let Some(task) = self.seek_task {
                        context.cancel(task);
                    }
                    self.seek_task = None;
                    self.seek_waiting = false;
                    self.selected_preset = None;
                    self.seek_baseline.clear();
                    if self
                        .challenge
                        .as_ref()
                        .is_some_and(|challenge| challenge.id == challenge_id)
                    {
                        self.challenge = None;
                    }
                    self.clear_pending_action();
                    self.accepted_challenge = None;
                    self.reconcile_accepted_challenge = false;
                    self.notice = Some("Challenge accepted. Opening the board.".to_owned());
                    self.open_board(context, summary.session());
                } else if seek_match {
                    if self.seek_candidate.is_none() {
                        self.seek_candidate = Some(summary);
                    }
                    let description = self.selected_preset.map_or_else(
                        || "selected preset".to_owned(),
                        |preset| format!("{} {}", preset.label(), preset.speed_label()),
                    );
                    self.notice = Some(format!(
                        "A matching {description} game started. Confirm it before closing the global seek."
                    ));
                } else if self.route == Route::Play {
                    self.notice =
                        Some("A Lichess game started. Open it from Ongoing games.".to_owned());
                }
            }
            Event::GameFinish(id) => {
                let current = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.game_id == id);
                if current && !self.board_is_live(&id) {
                    self.notice =
                        Some("The game finished while its board stream was paused.".to_owned());
                    self.discard_game(context, &id);
                } else if self.game.as_ref().is_some_and(|game| game.id == id) {
                    self.notice = Some("Lichess reports that the game finished.".to_owned());
                    if self.board_is_live(&id) {
                        self.next_board(context, &id);
                    }
                }
                self.summaries.retain(|summary| summary.id != id);
            }
            Event::Challenge(challenge) => {
                if challenge.direction == ChallengeDirection::Incoming {
                    if self
                        .accepted_challenge
                        .as_ref()
                        .is_some_and(|accepted| accepted.id != challenge.id)
                    {
                        self.notice = Some(
                            "Another challenge arrived while the accepted one is starting."
                                .to_owned(),
                        );
                    } else {
                        self.challenge = Some(challenge);
                    }
                    if self.route == Route::Play && self.accepted_challenge.is_none() {
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
                    if self
                        .accepted_challenge
                        .as_ref()
                        .is_some_and(|challenge| challenge.id == id)
                    {
                        self.accepted_challenge = None;
                        self.reconcile_accepted_challenge = false;
                    }
                    if matches!(
                        (&self.pending_action, &self.pending_scope),
                        (
                            Some(GameAction::AcceptChallenge(pending_id)),
                            Some(ActionScope::Challenge(scope_id))
                        ) if pending_id == &id && scope_id == &id
                    ) {
                        self.clear_pending_action();
                    }
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
                let Some(color) = self
                    .session
                    .as_ref()
                    .filter(|session| session.game_id == id)
                    .map(|session| session.color)
                else {
                    self.notice = Some("The board did not match the saved game.".to_owned());
                    self.close_board(context, id);
                    return;
                };
                if !self.selected_board_matches(context, id, &full) {
                    return;
                }
                let pending = self.pending_move.clone();
                let Some(game) = Game::from_full(full, color) else {
                    self.notice = Some("The server board could not be reconstructed.".to_owned());
                    self.close_board(context, id);
                    self.schedule_board_reconnect(context, id);
                    return;
                };
                self.game = Some(game);
                self.board_ready = true;
                self.board_backoff = 1;
                self.reconcile_pending_move();
                if pending.is_some() && self.pending_move.is_none() {
                    self.notice = Some("The board reconciled the pending move.".to_owned());
                } else if pending.is_some() {
                    self.pending_move = None;
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
                        self.board_ready = true;
                        self.reconcile_pending_move();
                        self.notice = None;
                        self.reset_clock(context, active);
                        self.finish_if_needed(context);
                    }
                    Some(ApplyState::Unchanged) => {
                        self.board_ready = true;
                    }
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
            BoardRecord::Unsupported(variant) => {
                self.notice = Some(format!(
                    "The {variant} variant is not supported; its reconnect state was cleared."
                ));
                self.discard_game(context, id);
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
        if game.id != pending.game_id {
            self.pending_move = None;
            return;
        }
        if game.state.moves.len() <= pending.at_ply {
            return;
        }
        if game.state.moves.get(pending.at_ply) == Some(&pending.movement) {
            self.pending_move = None;
            self.selected = None;
        } else {
            self.pending_move = None;
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
        self.deferred_board_open = None;
        self.clear_pending_action();
        self.pending_move = None;
        self.selected = None;
        self.clock.stop(context);
        self.close_board(context, &id);
        self.clear_board_rate_limit(context, &id);
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

    fn reconcile_accepted_challenge_from_playing(&mut self, context: &mut Context) -> bool {
        if self.suspended || !self.reconcile_accepted_challenge {
            return false;
        }
        self.reconcile_accepted_challenge = false;
        let Some(accepted) = self.accepted_challenge.take() else {
            return false;
        };
        let mut candidates = self
            .summaries
            .iter()
            .filter(|summary| accepted.matches_playing_game(summary));
        let first = candidates.next().cloned();
        let ambiguous = candidates.next().is_some();
        let recovered = (!ambiguous).then_some(first).flatten();
        if self
            .challenge
            .as_ref()
            .is_some_and(|challenge| challenge.id == accepted.id)
        {
            self.challenge = None;
        }
        if matches!(
            (&self.pending_action, &self.pending_scope),
            (
                Some(GameAction::AcceptChallenge(id)),
                Some(ActionScope::Challenge(scope_id))
            ) if id == &accepted.id && scope_id == &accepted.id
        ) {
            self.clear_pending_action();
        }
        if let Some(summary) = recovered {
            self.notice =
                Some("Recovered the accepted challenge from the current-game snapshot.".to_owned());
            self.open_board(context, summary.session());
            true
        } else {
            if self.route == Route::Challenge {
                self.route = Route::Play;
            }
            self.notice = Some(if ambiguous {
                "Several games matched the accepted challenge; choose the correct one from Ongoing games."
                    .to_owned()
            } else {
                "The accepted challenge was not active after reconnect; the wait was cleared."
                    .to_owned()
            });
            false
        }
    }

    fn recover_accepted_challenge(&mut self, context: &mut Context) {
        if self.accepted_challenge.is_some() {
            self.reconcile_accepted_challenge = true;
            if !self.suspended {
                let _ = self.refresh_playing(context);
            }
        }
    }

    fn clear_accepted_challenge_wait(&mut self) {
        let accepted = self.accepted_challenge.take().map(|challenge| challenge.id);
        self.reconcile_accepted_challenge = false;
        if self.challenge.as_ref().is_some_and(|challenge| {
            accepted
                .as_ref()
                .is_some_and(|accepted| accepted == &challenge.id)
        }) {
            self.challenge = None;
        }
        if matches!(
            (&self.pending_action, &self.pending_scope),
            (
                Some(GameAction::AcceptChallenge(id)),
                Some(ActionScope::Challenge(scope_id))
            ) if accepted.as_ref().is_some_and(|accepted| {
                accepted == id && accepted == scope_id
            })
        ) {
            self.clear_pending_action();
        }
        if self.route == Route::Challenge {
            self.route = Route::Play;
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
                    self.clear_pending_action();
                    self.accepted_challenge = None;
                    self.reconcile_accepted_challenge = false;
                    self.pending_move = None;
                    self.selected = None;
                    self.promotion = None;
                    self.confirmation = None;
                    if matches!(self.route, Route::Pairing | Route::Challenge | Route::Game) {
                        self.route = Route::Play;
                    }
                    self.close_live_reads(context);
                }
                Pending::EventOpen | Pending::EventNext => {
                    self.event_open = false;
                    self.recover_accepted_challenge(context);
                    let seconds = delay.unwrap_or(30).max(1);
                    self.set_event_rate_limit(context, seconds);
                    self.schedule_event_rate_wait(context, seconds);
                }
                Pending::Playing if self.accepted_challenge.is_some() => {
                    self.clear_accepted_challenge_wait();
                }
                Pending::BoardOpen(id) | Pending::BoardNext(id) => {
                    self.close_board(context, &id);
                    if self
                        .session
                        .as_ref()
                        .is_some_and(|session| session.game_id == id)
                    {
                        self.clock.stop(context);
                        self.deferred_board_open = None;
                        let seconds = delay.unwrap_or(30).max(1);
                        self.set_board_rate_limit(context, &id, seconds);
                        self.schedule_board_rate_wait(context, &id, seconds);
                    }
                }
                Pending::Seek { generation, preset }
                    if self.seek_is_current(generation, preset) =>
                {
                    self.seek_task = None;
                    self.seek_waiting = false;
                    self.selected_preset = None;
                    self.seek_baseline.clear();
                    self.seek_candidate = None;
                    self.clock.stop(context);
                    self.route = Route::Play;
                    let seconds = delay.unwrap_or(30).max(1);
                    self.set_seek_rate_limit(context, seconds);
                    self.schedule_seek_rate_wait(context, seconds);
                }
                Pending::SeekReconcile { generation }
                    if self.seek_waiting && self.seek_generation == generation =>
                {
                    let seconds = delay.unwrap_or(30).max(1);
                    self.retry_seek_reconciliation(
                        context,
                        generation,
                        format!(
                            "Current games are rate-limited for {seconds}s. Pairing remains cancelled server-side; reconciliation will retry."
                        ),
                        seconds,
                    );
                }
                Pending::Action {
                    action,
                    scope,
                    generation,
                } if self.pending_action_is(&action, &scope, generation) => {
                    self.clear_pending_action();
                    self.pending_move = None;
                    if matches!(scope, ActionScope::Challenge(_)) {
                        self.accepted_challenge = None;
                    }
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
                    let _ = self.refresh_playing(context);
                    self.open_event_stream(context);
                    if let Some(remaining) = self
                        .seek_rate_remaining()
                        .filter(|remaining| *remaining > 0)
                    {
                        self.schedule_seek_rate_wait(context, remaining);
                    }
                } else {
                    self.account = AccountState::Failed(
                        "Lichess returned an account response this client cannot read.".to_owned(),
                    );
                }
            }
            Pending::AccountRetry => self.validate_account(context),
            Pending::Playing => {
                if let Some(games) = api::parse_playing(bytes) {
                    self.playing_ready = true;
                    self.summaries = games;
                    let recovered = self.reconcile_accepted_challenge_from_playing(context);
                    if !recovered {
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
                                self.discard_game(context, &session.game_id);
                            }
                        }
                    }
                } else {
                    self.playing_ready = false;
                    if self.accepted_challenge.is_some() || self.reconcile_accepted_challenge {
                        self.clear_accepted_challenge_wait();
                    }
                    self.notice =
                        Some("Lichess returned a game list this client cannot read.".to_owned());
                }
                self.open_event_stream(context);
            }
            Pending::Puzzle => {
                if self.accept_puzzles(bytes) {
                    context.store().save(PUZZLE_KEY, self.encode_puzzles());
                    if matches!(
                        self.route,
                        Route::Puzzles | Route::Solve | Route::PuzzleResult
                    ) {
                        self.route = Route::Puzzles;
                    } else {
                        self.notice = Some("A 32-puzzle session is ready offline.".to_owned());
                    }
                } else {
                    self.notice =
                        Some("Lichess returned a puzzle batch this client cannot read.".to_owned());
                }
            }
            Pending::EventOpen => {
                if bytes.is_empty() {
                    self.event_open = true;
                    self.deferred_event_open = false;
                    self.clear_event_rate_limit(context);
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
                    self.recover_accepted_challenge(context);
                    self.close_event(context);
                }
            }
            Pending::EventRetry => self.open_event_stream(context),
            Pending::EventRateWait { remaining } => {
                self.schedule_event_rate_wait(context, remaining);
            }
            Pending::EventClose => self.schedule_event_reconnect(context),
            Pending::Seek { generation, preset } => {
                if self.route == Route::Pairing && self.seek_is_current(generation, preset) {
                    self.await_seek_event(
                        context,
                        generation,
                        preset,
                        "Waiting for match.".to_owned(),
                    );
                } else if self.seek_generation == generation {
                    self.seek_task = None;
                }
            }
            Pending::SeekGrace { generation } => {
                self.reconcile_ended_seek(context, generation);
            }
            Pending::SeekReconcile { generation } => {
                if self.seek_waiting && self.seek_generation == generation {
                    if let Some(games) = api::parse_playing(bytes) {
                        self.finish_seek_reconciliation(context, generation, games);
                    } else {
                        self.notice = Some(
                            "The current-game response could not be read. Pairing was not replayed; cancel or wait for another check."
                                .to_owned(),
                        );
                        let _ = self.spawn(
                            context,
                            Pending::SeekGrace { generation },
                            Task::Sleep { seconds: 10 },
                            false,
                        );
                    }
                }
            }
            Pending::SeekRateWait { remaining } => {
                self.schedule_seek_rate_wait(context, remaining);
            }
            Pending::BoardOpen(id) => {
                if self
                    .session
                    .as_ref()
                    .is_none_or(|session| session.game_id != id)
                {
                    self.close_board(context, &id);
                    return;
                }
                if bytes.is_empty() {
                    self.board_open = Some(id.clone());
                    self.board_ready = false;
                    self.board_backoff = 1;
                    self.next_board(context, &id);
                } else {
                    self.notice = Some("The board stream did not open cleanly.".to_owned());
                }
            }
            Pending::BoardNext(id) => {
                if self
                    .session
                    .as_ref()
                    .is_none_or(|session| session.game_id != id)
                {
                    self.close_board(context, &id);
                    return;
                }
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
            Pending::BoardRateWait { id, remaining } => {
                self.schedule_board_rate_wait(context, &id, remaining);
            }
            Pending::BoardClose(id) => {
                if self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.game_id == id)
                    && self.board_open.is_none()
                    && self.board_rate_remaining(&id).unwrap_or(0) == 0
                {
                    self.schedule_board_reconnect(context, &id);
                }
            }
            Pending::Action {
                action,
                scope,
                generation,
            } => {
                if !self.pending_action_is(&action, &scope, generation) {
                    return;
                }
                if !self.action_scope_is_current(&scope) {
                    self.clear_pending_action();
                    if matches!(scope, ActionScope::Challenge(_)) {
                        self.accepted_challenge = None;
                    }
                    if self.pending_move.as_ref().is_some_and(
                        |pending| matches!(&scope, ActionScope::Game(id) if id == &pending.game_id),
                    ) {
                        self.pending_move = None;
                    }
                    return;
                }
                match action {
                    GameAction::DeclineChallenge(id) => {
                        if self
                            .challenge
                            .as_ref()
                            .is_some_and(|challenge| challenge.id == id)
                        {
                            self.challenge = None;
                            self.route = Route::Play;
                        }
                        self.clear_pending_action();
                        self.accepted_challenge = None;
                    }
                    GameAction::Move(_)
                    | GameAction::AcceptChallenge(_)
                    | GameAction::Resign
                    | GameAction::Abort
                    | GameAction::OfferDraw
                    | GameAction::AcceptDraw
                    | GameAction::DeclineDraw
                    | GameAction::ClaimVictory => self.clear_pending_action(),
                }
                self.notice = Some(
                    "Lichess accepted the request; waiting for the stream to confirm it."
                        .to_owned(),
                );
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "failure handling keeps each asynchronous operation's recovery policy exhaustive"
    )]
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
                self.recover_accepted_challenge(context);
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
                    self.discard_game(context, &id);
                } else {
                    self.notice = Some(Failure::of(error).naming(api::SECRET));
                    self.schedule_board_reconnect(context, &id);
                }
            }
            Pending::Seek { generation, preset } => {
                if self.route == Route::Pairing && self.seek_is_current(generation, preset) {
                    self.await_seek_event(
                        context,
                        generation,
                        preset,
                        format!(
                            "{} Waiting briefly for gameStart; the seek was not replayed.",
                            Failure::of(error).naming(api::SECRET)
                        ),
                    );
                } else if self.seek_generation == generation {
                    self.seek_task = None;
                }
            }
            Pending::SeekReconcile { generation }
                if self.seek_waiting && self.seek_generation == generation =>
            {
                self.retry_seek_reconciliation(
                    context,
                    generation,
                    format!(
                        "{} Pairing was not replayed; current-game reconciliation will retry.",
                        Failure::of(error).naming(api::SECRET)
                    ),
                    10,
                );
            }
            Pending::Action {
                action,
                scope,
                generation,
            } => {
                if !self.pending_action_is(&action, &scope, generation) {
                    return;
                }
                let current = self.action_scope_is_current(&scope);
                let accepting = matches!(action, GameAction::AcceptChallenge(_));
                self.clear_pending_action();
                if accepting {
                    self.recover_accepted_challenge(context);
                } else if matches!(scope, ActionScope::Challenge(_)) {
                    self.accepted_challenge = None;
                }
                self.notice = Some(format!(
                    "{} The action was not replayed; the board is being reconciled.",
                    Failure::of(error).naming(api::SECRET)
                ));
                if !matches!(action, GameAction::Move(_)) || !current {
                    self.pending_move = None;
                }
                if let ActionScope::Game(id) = scope {
                    if self.game.as_ref().is_some_and(|game| game.id == id) {
                        self.close_board(context, &id);
                        self.schedule_board_reconnect(context, &id);
                    }
                }
            }
            Pending::Puzzle => {
                self.notice = Some(Failure::of(error).advice.to_owned());
            }
            Pending::Playing => {
                if self.accepted_challenge.is_some() || self.reconcile_accepted_challenge {
                    self.clear_accepted_challenge_wait();
                }
                self.notice = Some(Failure::of(error).naming(api::SECRET));
                self.open_event_stream(context);
            }
            Pending::EventRetry
            | Pending::EventRateWait { .. }
            | Pending::EventClose
            | Pending::SeekGrace { .. }
            | Pending::SeekReconcile { .. }
            | Pending::SeekRateWait { .. }
            | Pending::BoardRetry(_)
            | Pending::BoardRateWait { .. }
            | Pending::BoardClose(_)
            | Pending::Account => {}
            Pending::AccountRetry => self.schedule_account_retry(context),
        }
    }

    fn handle_cancelled(&mut self, context: &mut Context, pending: &Pending) {
        match pending {
            Pending::Seek { generation, .. } if self.seek_generation == *generation => {
                self.seek_task = None;
                self.seek_waiting = false;
                self.selected_preset = None;
                self.seek_baseline.clear();
                self.seek_candidate = None;
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
                self.board_ready = false;
            }
            Pending::Action {
                action,
                scope,
                generation,
            } if self.pending_action_is(action, scope, *generation) => {
                let current = self.action_scope_is_current(scope);
                let accepting = matches!(action, GameAction::AcceptChallenge(_));
                self.clear_pending_action();
                if accepting {
                    self.recover_accepted_challenge(context);
                } else if matches!(scope, ActionScope::Challenge(_)) {
                    self.accepted_challenge = None;
                }
                if !matches!(action, GameAction::Move(_)) || !current {
                    self.pending_move = None;
                }
                self.notice = Some(
                    "The action was cancelled with an unknown server outcome; reconnecting."
                        .to_owned(),
                );
                if let ActionScope::Game(id) = scope {
                    if self
                        .game
                        .as_ref()
                        .is_some_and(|game| game.id == id.as_str())
                    {
                        let id = id.clone();
                        self.close_board(context, &id);
                        self.schedule_board_reconnect(context, &id);
                    }
                }
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
        self.puzzles
            .iter()
            .skip(self.current_puzzle)
            .filter(|puzzle| puzzle.cursor < puzzle.solution.len())
            .count()
    }

    fn close_live_reads(&mut self, context: &mut Context) {
        if self.accepted_challenge.is_some() {
            self.reconcile_accepted_challenge = true;
        }
        if self.event_open || self.deferred_event_next {
            self.deferred_event_close = true;
        }
        if let Some(id) = self
            .board_open
            .clone()
            .or_else(|| self.deferred_board_next.clone())
        {
            self.deferred_board_closes.insert(id);
        }
        for (task, pending) in self.tasks.clone() {
            if matches!(
                pending,
                Pending::Account
                    | Pending::AccountRetry
                    | Pending::Playing
                    | Pending::Puzzle
                    | Pending::EventOpen
                    | Pending::EventNext
                    | Pending::EventRetry
                    | Pending::EventRateWait { .. }
                    | Pending::BoardOpen(_)
                    | Pending::BoardNext(_)
                    | Pending::BoardRetry(_)
                    | Pending::BoardRateWait { .. }
                    | Pending::Seek { .. }
                    | Pending::SeekGrace { .. }
                    | Pending::SeekReconcile { .. }
                    | Pending::SeekRateWait { .. }
            ) {
                context.cancel(task);
                self.tasks.remove(&task);
                self.retired_tasks.insert(task, pending);
            }
        }
        self.seek_task = None;
        self.seek_waiting = false;
        self.selected_preset = None;
        self.seek_baseline.clear();
        self.seek_candidate = None;
        self.event_open = false;
        self.deferred_event_open = matches!(self.account, AccountState::Ready(_));
        self.deferred_event_next = false;
        self.board_open = None;
        self.board_ready = false;
        self.deferred_board_next = None;
        self.clock.stop(context);
        self.flush_deferred_stream_closes(context);
    }

    #[cfg(debug_assertions)]
    #[allow(
        clippy::too_many_lines,
        reason = "named simulator fixtures keep each review screen deterministic and self-contained"
    )]
    fn install_demo(&mut self, scenario: &str) -> bool {
        match scenario {
            "home" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.event_open = true;
                self.playing_ready = true;
                self.route = Route::Home;
            }
            "home-disabled" => {
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
                self.board_open = Some("demoAB12".to_owned());
                self.board_ready = true;
                self.route = Route::Game;
            }
            "pairing" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.event_open = true;
                self.seek_task = Some(TaskId(999));
                self.seek_waiting = true;
                self.seek_generation = 1;
                self.selected_preset = Some(SeekPreset::Rapid10_5);
                self.route = Route::Pairing;
            }
            "candidate" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.event_open = true;
                self.playing_ready = true;
                self.seek_task = Some(TaskId(999));
                self.seek_waiting = true;
                self.seek_generation = 1;
                self.selected_preset = Some(SeekPreset::Rapid10_5);
                self.seek_candidate = Some(GameSummary {
                    id: "demoCD34".to_owned(),
                    color: Color::White,
                    opponent: "KnightReader".to_owned(),
                    rated: true,
                    is_my_turn: true,
                    last_move: None,
                    source: Some("lobby".to_owned()),
                    speed: Some("rapid".to_owned()),
                    variant: Some("standard".to_owned()),
                    seconds_left: Some(600),
                });
                self.route = Route::Pairing;
            }
            "reconciling" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.event_open = true;
                self.playing_ready = true;
                self.seek_waiting = true;
                self.seek_generation = 1;
                self.selected_preset = Some(SeekPreset::Rapid10_5);
                self.tasks
                    .insert(TaskId(998), Pending::SeekReconcile { generation: 1 });
                self.notice = Some("Checking games.".to_owned());
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
        self.loaded_board_rate = true;
        self.loaded_event_rate = true;
        self.loaded_seek_rate = true;
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
        context.store().load(BOARD_RATE_KEY);
        context.store().load(EVENT_RATE_KEY);
        context.store().load(SEEK_RATE_KEY);
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
            } else if key == BOARD_RATE_KEY {
                self.loaded_board_rate = true;
                if let Some(value) = value {
                    if let Some(limits) = decode_board_rate_limits(&value) {
                        self.board_rate_limits = limits;
                    } else {
                        context.store().forget(BOARD_RATE_KEY);
                        self.notice =
                            Some("Corrupted board retry metadata was discarded.".to_owned());
                    }
                }
            } else if key == EVENT_RATE_KEY {
                self.loaded_event_rate = true;
                if let Some(value) = value {
                    if let Some(not_before) = decode_rate_deadline(&value) {
                        self.event_rate_limit = Some(not_before);
                    } else {
                        context.store().forget(EVENT_RATE_KEY);
                        self.notice =
                            Some("Corrupted event retry metadata was discarded.".to_owned());
                    }
                }
            } else if key == SEEK_RATE_KEY {
                self.loaded_seek_rate = true;
                if let Some(value) = value {
                    if let Some(not_before) = decode_rate_deadline(&value) {
                        self.seek_rate_limit = Some(not_before);
                    } else {
                        context.store().forget(SEEK_RATE_KEY);
                        self.notice =
                            Some("Corrupted pairing retry metadata was discarded.".to_owned());
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
            } else if self.pending_action.is_some()
                || self.pending_move.is_some()
                || self.accepted_challenge.is_some()
            {
                self.notice = Some(
                    "Wait for the current server action before leaving this screen.".to_owned(),
                );
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
            self.home_page = 0;
            self.route = Route::Puzzles;
        } else if action == action_id("play") {
            self.home_page = 0;
            self.route = Route::Play;
            self.validate_account(context);
        } else if action == action_id("home-next") || action == action_id("home-previous") {
            let pages = self.home_pages(context).len();
            self.home_page = if action == action_id("home-next") {
                (self.home_page + 1).min(pages.saturating_sub(1))
            } else {
                self.home_page.saturating_sub(1)
            };
        } else if action == action_id("refresh-play") {
            self.validate_account(context);
        } else if action == action_id("quick-pair") {
            self.start_seek(context, SeekPreset::Rapid10_0);
        } else if let Some(preset) = seek_preset_action(action) {
            self.start_seek(context, preset);
        } else if action == action_id("cancel-seek") {
            self.cancel_seek(context);
        } else if action == action_id("open-seek-candidate") {
            self.open_seek_candidate(context);
        } else if action == action_id("keep-seeking") {
            self.seek_candidate = None;
            self.notice = Some("Continuing to wait on the existing seek.".to_owned());
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
            if let Some(session) = self.session.clone() {
                if self
                    .game
                    .as_ref()
                    .is_some_and(|game| !self.board_is_live(&game.id))
                {
                    self.open_board(context, session);
                } else {
                    self.route = Route::Game;
                }
            }
        } else if action == action_id("reconnect-board") {
            if let Some(session) = self.session.clone() {
                self.open_board(context, session);
            }
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
            let _ = self.refresh_playing(context);
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
            context.store().save(PUZZLE_KEY, self.encode_puzzles());
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
                Route::Solve => {
                    self.choose_puzzle_square(square);
                    context.store().save(PUZZLE_KEY, self.encode_puzzles());
                }
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
            if self.retired_tasks.remove(&task).is_some() && !self.suspended {
                if matches!(self.account, AccountState::Unknown)
                    && !self.has_pending(|pending| matches!(pending, Pending::Account))
                {
                    self.validate_account(context);
                } else if self.accepted_challenge.is_some() || self.reconcile_accepted_challenge {
                    let _ = self.refresh_playing(context);
                }
                self.flush_deferred_stream_closes(context);
                self.retry_deferred_board(context);
                self.retry_deferred_event(context);
            }
            return;
        };
        match outcome {
            TaskOutcome::Completed(bytes) => self.handle_completed(context, pending, &bytes),
            TaskOutcome::Failed(error) => self.handle_failed(context, pending, error),
            TaskOutcome::Cancelled => self.handle_cancelled(context, &pending),
        }
        self.flush_deferred_stream_closes(context);
        self.retry_deferred_board(context);
        self.retry_deferred_event(context);
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

fn seek_preset_action(action: ActionId) -> Option<SeekPreset> {
    SeekPreset::ALL
        .into_iter()
        .find(|preset| action == action_id(preset.action()))
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

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn decode_rate_deadline(bytes: &[u8]) -> Option<u64> {
    if bytes.len() > 1024 {
        return None;
    }
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if value.get("version")?.as_i64()? != 1 {
        return None;
    }
    let not_before = u64::try_from(value.get("not_before")?.as_i64()?).ok()?;
    (not_before <= unix_seconds().saturating_add(24 * 60 * 60)).then_some(not_before)
}

fn decode_board_rate_limits(bytes: &[u8]) -> Option<BTreeMap<String, u64>> {
    if bytes.len() > 16 * 1024 {
        return None;
    }
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if value.get("version")?.as_i64()? != 2 {
        return None;
    }
    let limits = value.get("limits")?.as_array()?;
    if limits.len() > 50 {
        return None;
    }
    let mut decoded = BTreeMap::new();
    for limit in limits {
        let id = limit.get("game_id")?.as_str()?;
        if !(8..=16).contains(&id.len()) || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return None;
        }
        let not_before = u64::try_from(limit.get("not_before")?.as_i64()?).ok()?;
        if not_before > unix_seconds().saturating_add(24 * 60 * 60)
            || decoded.insert(id.to_owned(), not_before).is_some()
        {
            return None;
        }
    }
    Some(decoded)
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
        api, board_cells, clock, AccountState, ActionScope, BoardRecord, Challenge,
        ChallengeDirection, ChallengeTime, Color, Event, FullGame, Game, GameAction, Lichess,
        Pending, Player, Puzzle, Route, ServerState, Session, ACCOUNT_RETRY_SECONDS,
        BOARD_RATE_KEY, EVENT_RATE_KEY, HOME_TILE_COUNT, SEEK_RATE_KEY,
    };
    use kobo_sdk::{
        action_id, ActionId, AppRunner, Command, Context, KoboApp, Screen, StoreRequest,
        TaskOutcome,
    };
    use kobo_ui::{
        render_with, tone, Chrome, ControlState, DisplayMetrics, Glyph, LayoutKind, Node, Surface,
        TextScale, TileShape, TileState, CLARA_BW_METRICS,
    };
    use std::collections::BTreeSet;

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
            board_open: Some(game.id.clone()),
            board_ready: true,
            game: Some(game),
            ..Lichess::default()
        }
    }

    fn ready_app() -> Lichess {
        Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        }
    }

    fn summary(id: &str, preset: api::SeekPreset) -> super::GameSummary {
        super::GameSummary {
            id: id.to_owned(),
            color: Color::White,
            opponent: "Other".to_owned(),
            rated: true,
            is_my_turn: true,
            last_move: None,
            source: Some("lobby".to_owned()),
            speed: Some(preset.speed().to_owned()),
            variant: Some("standard".to_owned()),
            seconds_left: Some(u32::from(preset.minutes()) * 60),
        }
    }

    fn painted(commands: Vec<Command>) -> Option<Screen> {
        commands.into_iter().find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen),
            _ => None,
        })
    }

    fn panels() -> Vec<(String, DisplayMetrics)> {
        kobo_profile::SUPPORTED_PROFILES
            .iter()
            .flat_map(|profile| {
                let portrait = DisplayMetrics {
                    width: i32::try_from(profile.width).expect("profile width"),
                    height: i32::try_from(profile.height).expect("profile height"),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale: TextScale::Default,
                };
                let landscape = DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                };
                [
                    (format!("{} portrait", profile.id), portrait),
                    (format!("{} landscape", profile.id), landscape),
                ]
            })
            .collect()
    }

    fn visible_tile_actions(screen: &Screen, metrics: &DisplayMetrics) -> BTreeSet<ActionId> {
        screen
            .layout_with(metrics, &Chrome::with_back(false))
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::Tile(action, _) => Some(action),
                _ => None,
            })
            .collect()
    }

    fn declared_tiles(nodes: &[Node]) -> Vec<(ActionId, TileShape, TileState, bool)> {
        let mut declared = Vec::new();
        for node in nodes {
            match node {
                Node::TileGrid { shape, tiles, .. } => declared.extend(
                    tiles
                        .iter()
                        .map(|tile| (tile.action, *shape, tile.state, tile.badge.is_empty())),
                ),
                Node::Band { slots, .. } => {
                    for slot in slots {
                        declared.extend(declared_tiles(&slot.nodes));
                    }
                }
                Node::Card { children, .. } => declared.extend(declared_tiles(children)),
                _ => {}
            }
        }
        declared
    }

    fn tile_copy(nodes: &[Node]) -> Vec<(ActionId, String, String)> {
        let mut copy = Vec::new();
        for node in nodes {
            match node {
                Node::TileGrid { tiles, .. } => copy.extend(
                    tiles
                        .iter()
                        .map(|tile| (tile.action, tile.label.clone(), tile.subtitle.clone())),
                ),
                Node::Band { slots, .. } => {
                    for slot in slots {
                        copy.extend(tile_copy(&slot.nodes));
                    }
                }
                Node::Card { children, .. } => copy.extend(tile_copy(children)),
                _ => {}
            }
        }
        copy
    }

    fn pixel(surface: &Surface, metrics: &DisplayMetrics, x: i32, y: i32) -> u8 {
        let x = usize::try_from(x).expect("pixel x");
        let y = usize::try_from(y).expect("pixel y");
        let width = usize::try_from(metrics.width).expect("surface width");
        surface.pixels[y * width + x]
    }

    fn assert_consistent_card_grid(layout: &kobo_ui::Layout, columns: usize) {
        let outlines = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::TileOutline(_)))
            .collect::<Vec<_>>();
        let first_y = outlines.first().expect("card outline").rect.y;
        let first_row = outlines
            .iter()
            .filter(|outline| outline.rect.y == first_y)
            .collect::<Vec<_>>();
        assert_eq!(first_row.len(), columns);
        let widths = first_row
            .iter()
            .map(|outline| outline.rect.width)
            .collect::<BTreeSet<_>>();
        assert!(
            widths.iter().next_back().expect("width") - widths.iter().next().expect("width") <= 2,
            "card widths were {widths:?}"
        );
        let mut horizontal = first_row
            .windows(2)
            .map(|pair| pair[1].rect.x - pair[0].rect.x - pair[0].rect.width)
            .collect::<Vec<_>>();
        horizontal.sort_unstable();
        assert!(horizontal.last().expect("gap") - horizontal.first().expect("gap") <= 1);
        let rows = outlines
            .iter()
            .map(|outline| outline.rect.y)
            .collect::<BTreeSet<_>>();
        let rows = rows.into_iter().collect::<Vec<_>>();
        let mut vertical = rows
            .windows(2)
            .map(|pair| pair[1] - pair[0] - outlines[0].rect.height)
            .collect::<Vec<_>>();
        vertical.sort_unstable();
        if let (Some(first), Some(last)) = (vertical.first(), vertical.last()) {
            assert!(last - first <= 1);
        }
        for outline in outlines {
            let content = layout.nodes.iter().filter(|node| {
                node.id == outline.id
                    && matches!(
                        node.kind,
                        LayoutKind::TileGlyph(_)
                            | LayoutKind::TileGlyphMuted(_)
                            | LayoutKind::TileLabel
                            | LayoutKind::TileLabelMuted
                            | LayoutKind::TileSubtitle
                    )
            });
            for node in content {
                assert!(node.rect.x > outline.rect.x);
                assert!(node.rect.y > outline.rect.y);
                assert!(node.rect.x + node.rect.width < outline.rect.x + outline.rect.width);
                assert!(node.rect.y + node.rect.height < outline.rect.y + outline.rect.height);
            }
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
    fn folio_home_pages_every_preset_without_overflow() {
        let expected = std::iter::once(action_id("puzzles"))
            .chain(
                api::SeekPreset::ALL
                    .into_iter()
                    .map(|preset| action_id(preset.action())),
            )
            .chain(std::iter::once(action_id("play")))
            .collect::<BTreeSet<_>>();
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(ready_app(), metrics);
            let mut screen = painted(runner.start()).expect("home screen");
            let mut seen = BTreeSet::new();
            for page in 0..HOME_TILE_COUNT {
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                for node in &layout.nodes {
                    assert!(
                        node.rect.x >= 0
                            && node.rect.y >= 0
                            && node.rect.x.saturating_add(node.rect.width) <= metrics.width
                            && node.rect.y.saturating_add(node.rect.height) <= metrics.height,
                        "{name}: {:?} overflowed on page {page}",
                        node.kind
                    );
                }
                let tiles = declared_tiles(&screen.nodes);
                assert!(tiles
                    .iter()
                    .all(|(_, shape, _, badge_empty)| *shape == TileShape::Card && *badge_empty));
                let declared = tiles
                    .iter()
                    .map(|(action, _, _, _)| *action)
                    .collect::<BTreeSet<_>>();
                let visible = visible_tile_actions(&screen, &metrics);
                assert_eq!(
                    visible, declared,
                    "{name}: page {page} declared a tile that was not laid out"
                );
                if page == 0 {
                    assert!(visible.contains(&action_id("play")));
                    assert!(visible.contains(&action_id("puzzles")));
                }
                seen.extend(visible);
                let Some(next) = painted(runner.action(action_id("home-next"))) else {
                    break;
                };
                screen = next;
            }
            assert_eq!(seen, expected, "{name}: a home tile was lost or duplicated");
        }
    }

    #[test]
    fn unready_home_uses_one_status_and_safe_tiles_without_close_accessories() {
        let mut runner = AppRunner::new(Lichess::default());
        let screen = painted(runner.start()).expect("home");
        assert!(format!("{screen:?}").contains("Offline"));
        let tiles = declared_tiles(&screen.nodes);
        assert!(tiles.iter().all(|(action, _, state, badge_empty)| {
            let utility = *action == action_id("play") || *action == action_id("puzzles");
            *badge_empty
                && if utility {
                    *state == TileState::Normal
                } else {
                    *state == TileState::Unavailable
                }
        }));
        assert!(!screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false))
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::TileGlyph(Glyph::Close)));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::TileOutline(ControlState::Disabled)));
        assert!(layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::TileGlyphMuted(_))));
        assert!(layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::TileLabelMuted));
        assert!(!layout
            .nodes
            .iter()
            .any(|node| { matches!(node.kind, LayoutKind::TileState(_) | LayoutKind::TileBadge) }));
        let outline = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TileOutline(ControlState::Disabled))
            .expect("disabled card outline");
        let mut surface = Surface::new(
            usize::try_from(CLARA_BW_METRICS.width).expect("width"),
            usize::try_from(CLARA_BW_METRICS.height).expect("height"),
        );
        render_with(
            &screen,
            &CLARA_BW_METRICS,
            &Chrome::with_back(false),
            &mut surface,
            None,
        );
        assert_ne!(
            pixel(
                &surface,
                &CLARA_BW_METRICS,
                outline.rect.x + outline.rect.width / 2,
                outline.rect.y
            ),
            tone::PAPER
        );
        let preset = layout
            .nodes
            .iter()
            .find(|node| {
                node.kind
                    == LayoutKind::Tile(
                        action_id(api::SeekPreset::Rapid10_0.action()),
                        ControlState::Disabled,
                    )
            })
            .expect("disabled preset");
        assert_eq!(
            layout.hit_test(
                preset.rect.x + preset.rect.width / 2,
                preset.rect.y + preset.rect.height / 2
            ),
            None
        );
    }

    #[test]
    fn home_copy_is_only_destination_or_time_and_speed() {
        let context = Context::default();
        let mut app = ready_app();
        for page in 0..app.home_pages(&context).len() {
            app.home_page = page;
            let screen = app.home(&context);
            let rendered = format!("{screen:?}");
            for forbidden in ["Rated", "random color", "protocol", "API"] {
                assert!(
                    !rendered.contains(forbidden),
                    "{forbidden} appeared on home"
                );
            }
            for (action, label, subtitle) in tile_copy(&screen.nodes) {
                if action == action_id("play") {
                    assert_eq!(label, "Account/Games");
                    assert_eq!(subtitle, "Challenges · boards");
                } else if action == action_id("puzzles") {
                    assert_eq!(label, "Puzzles");
                    assert_eq!(subtitle, "Offline training");
                } else {
                    let preset = api::SeekPreset::ALL
                        .into_iter()
                        .find(|preset| action == action_id(preset.action()))
                        .expect("preset action");
                    assert_eq!(label, preset.label());
                    assert_eq!(subtitle, preset.speed_label());
                }
            }
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one acceptance test ties card geometry, full hit targets, and rendered outlines across both Clara orientations"
    )]
    fn clara_home_is_dense_in_portrait_and_landscape() {
        for (name, metrics, minimum) in [
            ("portrait", CLARA_BW_METRICS, HOME_TILE_COUNT),
            (
                "landscape",
                DisplayMetrics {
                    width: CLARA_BW_METRICS.height,
                    height: CLARA_BW_METRICS.width,
                    ..CLARA_BW_METRICS
                },
                6,
            ),
        ] {
            let mut runner = AppRunner::with_metrics(ready_app(), metrics);
            let screen = painted(runner.start()).expect("home");
            let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
            let tiles = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(_, _)))
                .collect::<Vec<_>>();
            let outlines = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::TileOutline(_)))
                .collect::<Vec<_>>();
            let visible = visible_tile_actions(&screen, &metrics);
            assert!(
                visible.len() >= minimum,
                "Clara {name} showed only {} tiles",
                visible.len()
            );
            assert!(visible.contains(&action_id("play")));
            assert!(visible.contains(&action_id("puzzles")));
            assert_eq!(outlines.len(), tiles.len());
            assert!(tiles
                .iter()
                .all(|tile| outlines.iter().any(|outline| outline.rect == tile.rect)));
            assert!(tiles.iter().all(|tile| {
                tile.rect.width >= metrics.touch_target_minimum()
                    && tile.rect.height >= metrics.touch_target_minimum()
            }));
            for tile in &tiles {
                let LayoutKind::Tile(action, ControlState::Enabled) = tile.kind else {
                    panic!("ready tile was disabled")
                };
                for (x, y) in [
                    (tile.rect.x + 1, tile.rect.y + 1),
                    (
                        tile.rect.x + tile.rect.width - 2,
                        tile.rect.y + tile.rect.height - 2,
                    ),
                ] {
                    assert_eq!(
                        layout.hit_test(x, y),
                        Some(action),
                        "Clara {name} card did not own its full hit rectangle"
                    );
                }
            }
            let mut surface = Surface::new(
                usize::try_from(metrics.width).expect("width"),
                usize::try_from(metrics.height).expect("height"),
            );
            render_with(
                &screen,
                &metrics,
                &Chrome::with_back(false),
                &mut surface,
                None,
            );
            for outline in outlines {
                assert_ne!(
                    pixel(
                        &surface,
                        &metrics,
                        outline.rect.x + outline.rect.width / 2,
                        outline.rect.y
                    ),
                    tone::PAPER,
                    "Clara {name} card top edge did not render"
                );
                assert_ne!(
                    pixel(
                        &surface,
                        &metrics,
                        outline.rect.x,
                        outline.rect.y + outline.rect.height / 2
                    ),
                    tone::PAPER,
                    "Clara {name} card left edge did not render"
                );
            }
            if name == "portrait" {
                assert_consistent_card_grid(&layout, 3);
                assert_eq!(
                    tiles
                        .iter()
                        .map(|tile| tile.rect.x)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    3
                );
                assert_eq!(
                    tiles
                        .iter()
                        .map(|tile| tile.rect.y)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    3
                );
            } else {
                assert_consistent_card_grid(&layout, 3);
            }
        }
    }

    #[test]
    fn returning_from_account_games_restores_the_primary_home_page() {
        let mut app = ready_app();
        app.home_page = 2;
        let mut context = Context::default();
        app.on_action(&mut context, action_id("play"));
        assert_eq!(app.home_page, 0);
        assert_eq!(app.route, Route::Play);
        app.on_action(&mut context, ActionId::BACK);
        assert_eq!(app.route, Route::Home);
        assert_eq!(app.home_page, 0);
    }

    #[test]
    fn every_folio_preset_tile_launches_its_exact_seek() {
        for preset in api::SeekPreset::ALL {
            let mut runner = AppRunner::new(ready_app());
            runner.start();
            let commands = runner.action(action_id(preset.action()));
            let body = commands.iter().find_map(|command| match command {
                Command::Spawn {
                    work: kobo_sdk::Task::Post { url, body, .. },
                    ..
                } if url.ends_with("/api/board/seek") => Some(body),
                _ => None,
            });
            assert_eq!(
                body.map(String::as_str),
                Some(preset.body().as_str()),
                "{}",
                preset.label()
            );
            assert_eq!(runner.app().selected_preset, Some(preset));
            assert_eq!(runner.app().route, Route::Pairing);
        }
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
            game_id: "abcdEF12".to_owned(),
            movement: "e2e4".to_owned(),
            at_ply: 0,
        });
        app.pending_action = Some(GameAction::Move("e2e4".to_owned()));
        app.pending_scope = Some(ActionScope::Game("abcdEF12".to_owned()));
        app.pending_action_generation = Some(1);
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
        assert!(app.pending_action.is_some());
        app.handle_completed(
            &mut context,
            Pending::Action {
                action: GameAction::Move("e2e4".to_owned()),
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 1,
            },
            &[],
        );
        assert!(app.pending_action.is_none());
        assert_eq!(
            app.game.as_ref().expect("game").state.moves,
            ["e2e4".to_owned()]
        );
    }

    #[test]
    fn puzzle_resume_position_and_progress_survive_restart() {
        let second_fen = super::chess::play(super::chess::START, "d2d4")
            .expect("opening move")
            .0;
        let mut app = Lichess {
            puzzles: vec![
                Puzzle {
                    id: "first".to_owned(),
                    fen: super::chess::START.to_owned(),
                    solution: vec!["e2e4".to_owned()],
                    cursor: 0,
                },
                Puzzle {
                    id: "second".to_owned(),
                    fen: second_fen,
                    solution: vec!["d2d4".to_owned(), "d7d5".to_owned()],
                    cursor: 1,
                },
            ],
            current_puzzle: 1,
            puzzle_wrong: 1,
            ..Lichess::default()
        };
        let encoded = app.encode_puzzles();
        app.puzzles.clear();
        app.current_puzzle = 0;
        app.puzzle_wrong = 0;
        assert!(app.decode_puzzles(&encoded));
        assert_eq!(app.current_puzzle, 1);
        assert_eq!(app.puzzle_wrong, 1);
        assert_eq!(app.puzzles[1].cursor, 1);

        app.current_puzzle = 0;
        app.puzzles[0].cursor = app.puzzles[0].solution.len();
        let completed = app.encode_puzzles();
        assert!(app.decode_puzzles(&completed));
        assert_eq!(app.current_puzzle, 1);
        assert_eq!(app.remaining_puzzles(), 1);
    }

    #[test]
    fn stale_pre_fix_puzzle_position_is_not_restored() {
        let stale = kobo_json::ObjectBuilder::new()
            .set("id", "uOjyL")
            .set(
                "fen",
                "2r3k1/1b4bp/pp4p1/5pq1/2Pn4/PPN4P/1B3PP1/2RBQ1K1 w - - 2 21",
            )
            .set("solution", vec!["b7d5".to_owned(), "c4d5".to_owned()])
            .set("cursor", 0_u32)
            .build();
        assert!(Puzzle::from_stored(&stale).is_none());
    }

    #[test]
    fn puzzle_solver_auto_plays_forced_opponent_replies() {
        let mut app = Lichess {
            route: Route::Solve,
            puzzles: vec![Puzzle {
                id: "line".to_owned(),
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4", "e7e5", "g1f3"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                cursor: 0,
            }],
            ..Lichess::default()
        };
        app.choose_puzzle_square("e2".to_owned());
        app.choose_puzzle_square("e4".to_owned());
        assert_eq!(app.puzzles[0].cursor, 2);
        assert_eq!(super::chess::piece_at(&app.puzzles[0].fen, "e5"), Some('p'));
        assert_eq!(app.route, Route::Solve);
        assert!(app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("Opponent replied"));

        app.choose_puzzle_square("g1".to_owned());
        app.choose_puzzle_square("f3".to_owned());
        assert_eq!(app.puzzles[0].cursor, 3);
        assert_eq!(app.route, Route::PuzzleResult);
    }

    #[test]
    fn background_puzzle_download_does_not_interrupt_a_live_game() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        let response = format!(
            r#"{{"puzzles":[{{"game":{{}},"puzzle":{{"id":"puzzle1","fen":"{}","solution":["e2e4"],"initialPly":0}}}}]}}"#,
            super::chess::START
        );
        app.handle_completed(&mut context, Pending::Puzzle, response.as_bytes());
        assert_eq!(app.route, Route::Game);
        assert!(app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("ready offline"));
        assert_eq!(app.puzzles.len(), 1);
    }

    #[test]
    fn official_batch_position_includes_the_move_before_the_solution() {
        let value = kobo_json::parse(
            r#"{
                "game": {
                    "pgn": "c4 Nf6 Nc3 d6 e4 Bg4 f3 Bh5 h4 e6 g4 Be7 gxh5 Nxh5 d4 Bxh4+ Kd2 Bg5+ Kc2 Nf4 Nge2 Nxe2 Qxe2 Nc6 Qd3"
                },
                "puzzle": {
                    "id": "94Rds",
                    "initialPly": 24,
                    "solution": ["c6b4", "c2b3", "b4d3"]
                }
            }"#,
        )
        .expect("official puzzle example");
        let puzzle = Puzzle::read(&value).expect("playable puzzle");
        assert!(super::chess::legal(&puzzle.fen, &puzzle.solution[0]));
        assert_eq!(super::chess::side_to_move(&puzzle.fen), Some('b'));
    }

    #[test]
    fn puzzle_fen_is_read_from_the_puzzle_object_and_must_match_the_solution() {
        let value = kobo_json::parse(&format!(
            r#"{{
                "game": {{}},
                "puzzle": {{
                    "id": "direct-fen",
                    "initialPly": 0,
                    "fen": "{}",
                    "solution": ["e2e4"]
                }}
            }}"#,
            super::chess::START
        ))
        .expect("puzzle with direct fen");
        assert!(Puzzle::read(&value).is_some());

        let invalid = kobo_json::parse(&format!(
            r#"{{
                "game": {{}},
                "puzzle": {{
                    "id": "invalid-fen",
                    "initialPly": 0,
                    "fen": "{}",
                    "solution": ["e7e5"]
                }}
            }}"#,
            super::chess::START
        ))
        .expect("puzzle with mismatched solution");
        assert!(Puzzle::read(&invalid).is_none());
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
    fn seek_retry_after_disables_immediate_duplicate_pairing() {
        let mut app = Lichess {
            route: Route::Pairing,
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            playing_ready: true,
            seek_waiting: true,
            seek_generation: 1,
            selected_preset: Some(api::SeekPreset::Rapid10_0),
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.handle_completed(
            &mut context,
            Pending::Seek {
                generation: 1,
                preset: api::SeekPreset::Rapid10_0,
            },
            b"COBALT-HTTP/1 429\nRetry-After: 27\n\n",
        );
        assert_eq!(app.route, Route::Play);
        assert!(app.seek_rate_remaining().is_some_and(|seconds| seconds > 0));
        assert!(app.has_pending(|pending| { matches!(pending, Pending::SeekRateWait { .. }) }));
        assert!(context.commands().iter().any(|command| {
            matches!(
                command,
                Command::Store(StoreRequest::Save { key, .. }) if key == SEEK_RATE_KEY
            )
        }));
        let before = context
            .commands()
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    Command::Spawn {
                        work: kobo_sdk::Task::Post { url, .. },
                        ..
                    } if url.ends_with("/api/board/seek")
                )
            })
            .count();
        app.start_seek(&mut context, api::SeekPreset::Rapid10_0);
        let after = context
            .commands()
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    Command::Spawn {
                        work: kobo_sdk::Task::Post { url, .. },
                        ..
                    } if url.ends_with("/api/board/seek")
                )
            })
            .count();
        assert_eq!(after, before);
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
                source: Some("lobby".to_owned()),
                speed: Some("rapid".to_owned()),
                variant: Some("standard".to_owned()),
                seconds_left: Some(600),
            }],
            event_open: true,
            playing_ready: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.start_seek(&mut context, api::SeekPreset::Rapid10_0);
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
    fn selected_preset_filters_candidates_and_full_board_increment() {
        let mut app = ready_app();
        app.route = Route::Play;
        let mut context = Context::default();
        app.start_seek(&mut context, api::SeekPreset::Rapid10_5);
        app.handle_event(
            &mut context,
            Event::GameStart(summary("wrongTime", api::SeekPreset::Rapid15_10)),
        );
        assert!(app.seek_candidate.is_none());
        app.handle_event(
            &mut context,
            Event::GameStart(summary("rightClk", api::SeekPreset::Rapid10_5)),
        );
        assert_eq!(
            app.seek_candidate.as_ref().map(|game| game.id.as_str()),
            Some("rightClk")
        );
        app.open_seek_candidate(&mut context);
        assert_eq!(
            app.expected_seek_game,
            Some(("rightClk".to_owned(), api::SeekPreset::Rapid10_5))
        );
        app.handle_board(
            &mut context,
            "rightClk",
            BoardRecord::Full(FullGame {
                id: "rightClk".to_owned(),
                initial_fen: "startpos".to_owned(),
                rated: true,
                speed: "blitz".to_owned(),
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
                    moves: Vec::new(),
                    white_ms: 300_000,
                    black_ms: 300_000,
                    white_increment_ms: 0,
                    black_increment_ms: 0,
                    status: "started".to_owned(),
                    winner: None,
                    white_draw: false,
                    black_draw: false,
                    white_takeback: false,
                    black_takeback: false,
                },
            }),
        );
        assert!(app.game.is_none());
        assert_eq!(app.route, Route::Play);
        assert!(app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("not the selected 10+5 Rapid clock"));
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
    fn accepted_challenge_waits_for_the_matching_opponent_game() {
        let challenge = Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
            direction: ChallengeDirection::Incoming,
            status: "created".to_owned(),
            rated: false,
            variant: "standard".to_owned(),
            speed: "rapid".to_owned(),
            time_control: ChallengeTime::Clock {
                initial_seconds: Some(600),
                increment_seconds: Some(0),
            },
        };
        let mut app = Lichess {
            route: Route::Challenge,
            challenge: Some(challenge.clone()),
            accepted_challenge: Some(challenge),
            pending_action: Some(GameAction::AcceptChallenge("chall123".to_owned())),
            pending_scope: Some(ActionScope::Challenge("chall123".to_owned())),
            pending_action_generation: Some(1),
            ..Lichess::default()
        };
        let mut context = Context::default();
        let second_challenge = api::parse_event(
            br#"{"type":"challenge","challenge":{"id":"other456","status":"created","direction":"in","challenger":{"name":"SomeoneElse"},"rated":false,"variant":{"key":"standard"},"speed":"rapid","timeControl":{"type":"clock","limit":600,"increment":0}}}"#,
        )
        .expect("second challenge");
        app.handle_event(&mut context, second_challenge);
        assert_eq!(
            app.challenge
                .as_ref()
                .map(|challenge| challenge.id.as_str()),
            Some("chall123")
        );
        let unrelated = api::parse_event(
            br#"{"type":"gameStart","game":{"gameId":"other123","color":"white","rated":false,"speed":"rapid","source":"friend","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"SomeoneElse"}}}"#,
        )
        .expect("unrelated friend game");
        app.handle_event(&mut context, unrelated);
        assert!(app.challenge.is_some());
        assert!(matches!(
            app.pending_action,
            Some(GameAction::AcceptChallenge(_))
        ));
        assert_eq!(app.route, Route::Challenge);

        let matched = api::parse_event(
            br#"{"type":"gameStart","game":{"gameId":"match123","color":"black","rated":false,"speed":"rapid","source":"friend","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":false,"lastMove":"","opponent":{"username":"ReaderTwo"}}}"#,
        )
        .expect("accepted challenge game");
        app.handle_event(&mut context, matched);
        assert!(app.challenge.is_none());
        assert!(app.pending_action.is_none());
        assert_eq!(app.route, Route::Game);
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("match123")
        );
    }

    #[test]
    fn accepted_challenge_recovers_from_playing_snapshot_after_suspend() {
        let challenge = Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
            direction: ChallengeDirection::Incoming,
            status: "created".to_owned(),
            rated: false,
            variant: "standard".to_owned(),
            speed: "rapid".to_owned(),
            time_control: ChallengeTime::Clock {
                initial_seconds: Some(600),
                increment_seconds: Some(0),
            },
        };
        let mut app = Lichess {
            route: Route::Challenge,
            challenge: Some(challenge.clone()),
            accepted_challenge: Some(challenge),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.handle_completed(
            &mut context,
            Pending::Playing,
            br#"{"nowPlaying":[{"gameId":"match123","color":"black","rated":false,"source":"friend","speed":"rapid","variant":{"key":"standard"},"secondsLeft":480,"isMyTurn":false,"lastMove":"","opponent":{"username":"ReaderTwo"}}]}"#,
        );
        assert!(app.accepted_challenge.is_none());
        assert_eq!(app.route, Route::Game);
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("match123")
        );

        let mut missing = Lichess {
            route: Route::Challenge,
            accepted_challenge: Some(Challenge {
                id: "chall123".to_owned(),
                challenger: "ReaderTwo".to_owned(),
                direction: ChallengeDirection::Incoming,
                status: "created".to_owned(),
                rated: false,
                variant: "standard".to_owned(),
                speed: "rapid".to_owned(),
                time_control: ChallengeTime::Clock {
                    initial_seconds: Some(600),
                    increment_seconds: Some(0),
                },
            }),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        missing.handle_completed(
            &mut Context::default(),
            Pending::Playing,
            br#"{"nowPlaying":[]}"#,
        );
        assert!(missing.accepted_challenge.is_none());
        assert_eq!(missing.route, Route::Play);

        let mut ambiguous = Lichess {
            route: Route::Challenge,
            accepted_challenge: Some(Challenge {
                id: "chall123".to_owned(),
                challenger: "ReaderTwo".to_owned(),
                direction: ChallengeDirection::Incoming,
                status: "created".to_owned(),
                rated: false,
                variant: "standard".to_owned(),
                speed: "rapid".to_owned(),
                time_control: ChallengeTime::Clock {
                    initial_seconds: Some(600),
                    increment_seconds: Some(0),
                },
            }),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        ambiguous.handle_completed(
            &mut Context::default(),
            Pending::Playing,
            br#"{"nowPlaying":[{"gameId":"match123","color":"black","rated":false,"source":"friend","speed":"rapid","variant":{"key":"standard"},"secondsLeft":480,"isMyTurn":false,"lastMove":"","opponent":{"username":"ReaderTwo"}},{"gameId":"match456","color":"white","rated":false,"source":"friend","speed":"rapid","variant":{"key":"standard"},"secondsLeft":420,"isMyTurn":true,"lastMove":"e7e5","opponent":{"username":"ReaderTwo"}}]}"#,
        );
        assert!(ambiguous.session.is_none());
        assert_eq!(ambiguous.route, Route::Play);
        assert!(ambiguous
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("Several games matched"));
    }

    #[test]
    fn accepted_challenge_rate_limit_clears_the_navigation_lock() {
        let challenge = Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
            direction: ChallengeDirection::Incoming,
            status: "created".to_owned(),
            rated: false,
            variant: "standard".to_owned(),
            speed: "rapid".to_owned(),
            time_control: ChallengeTime::Clock {
                initial_seconds: Some(600),
                increment_seconds: Some(0),
            },
        };
        let mut app = Lichess {
            route: Route::Challenge,
            challenge: Some(challenge.clone()),
            accepted_challenge: Some(challenge),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        app.handle_completed(
            &mut Context::default(),
            Pending::Playing,
            b"COBALT-HTTP/1 429\nRetry-After: 19\n\n",
        );
        assert!(app.accepted_challenge.is_none());
        assert!(app.challenge.is_none());
        assert_eq!(app.route, Route::Play);
        assert!(app.notice.as_deref().unwrap_or_default().contains("19s"));
    }

    #[test]
    fn ambiguous_accept_challenge_failure_reconciles_playing_games() {
        let challenge = Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
            direction: ChallengeDirection::Incoming,
            status: "created".to_owned(),
            rated: false,
            variant: "standard".to_owned(),
            speed: "rapid".to_owned(),
            time_control: ChallengeTime::Clock {
                initial_seconds: Some(600),
                increment_seconds: Some(0),
            },
        };
        let mut app = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            challenge: Some(challenge.clone()),
            accepted_challenge: Some(challenge),
            pending_action: Some(GameAction::AcceptChallenge("chall123".to_owned())),
            pending_scope: Some(ActionScope::Challenge("chall123".to_owned())),
            pending_action_generation: Some(1),
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.handle_failed(
            &mut context,
            Pending::Action {
                action: GameAction::AcceptChallenge("chall123".to_owned()),
                scope: ActionScope::Challenge("chall123".to_owned()),
                generation: 1,
            },
            kobo_sdk::TaskError::Unreachable,
        );
        assert!(app.accepted_challenge.is_some());
        assert!(app.reconcile_accepted_challenge);
        assert!(app.has_pending(|pending| matches!(pending, Pending::Playing)));
    }

    #[test]
    fn failed_challenge_reconciliation_always_unblocks_navigation() {
        let challenge = || Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
            direction: ChallengeDirection::Incoming,
            status: "created".to_owned(),
            rated: false,
            variant: "standard".to_owned(),
            speed: "rapid".to_owned(),
            time_control: ChallengeTime::Clock {
                initial_seconds: Some(600),
                increment_seconds: Some(0),
            },
        };
        for malformed in [false, true] {
            let mut app = Lichess {
                route: Route::Challenge,
                challenge: Some(challenge()),
                accepted_challenge: Some(challenge()),
                reconcile_accepted_challenge: true,
                ..Lichess::default()
            };
            let mut context = Context::default();
            if malformed {
                app.handle_completed(&mut context, Pending::Playing, b"{broken");
            } else {
                app.handle_failed(
                    &mut context,
                    Pending::Playing,
                    kobo_sdk::TaskError::Unreachable,
                );
            }
            assert!(app.accepted_challenge.is_none());
            assert!(!app.reconcile_accepted_challenge);
            assert_eq!(app.route, Route::Play);
        }
    }

    #[test]
    fn event_stream_rate_limit_reopens_after_persisted_retry_after() {
        let mut app = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            accepted_challenge: Some(Challenge {
                id: "chall123".to_owned(),
                challenger: "ReaderTwo".to_owned(),
                direction: ChallengeDirection::Incoming,
                status: "created".to_owned(),
                rated: false,
                variant: "standard".to_owned(),
                speed: "rapid".to_owned(),
                time_control: ChallengeTime::Clock {
                    initial_seconds: Some(600),
                    increment_seconds: Some(0),
                },
            }),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.handle_completed(
            &mut context,
            Pending::EventOpen,
            b"COBALT-HTTP/1 429\nRetry-After: 21\n\n",
        );
        assert!(!app.event_open);
        assert!(app.accepted_challenge.is_some());
        assert!(app.has_pending(|pending| matches!(pending, Pending::Playing)));
        assert!(app.event_rate_limit.is_some());
        assert!(app
            .has_pending(|pending| { matches!(pending, Pending::EventRateWait { remaining: 0 }) }));
        let saved = context.commands().iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == EVENT_RATE_KEY => {
                super::decode_rate_deadline(value)
            }
            _ => None,
        });
        assert!(saved.is_some_and(|not_before| not_before >= super::unix_seconds()));

        app.close_live_reads(&mut context);
        let mut resumed = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_rate_limit: saved,
            ..Lichess::default()
        };
        resumed.open_event_stream(&mut Context::default());
        assert!(resumed.has_pending(|pending| { matches!(pending, Pending::EventRateWait { .. }) }));
        assert!(!resumed.has_pending(|pending| matches!(pending, Pending::EventOpen)));

        let mut capacity = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_rate_limit: Some(super::unix_seconds() + 20),
            ..Lichess::default()
        };
        let mut full = Context::default();
        for _ in 0..4 {
            assert!(full.spawn(kobo_sdk::Task::Sleep { seconds: 30 }).is_some());
        }
        capacity.open_event_stream(&mut full);
        assert!(capacity.deferred_event_open);
        capacity.retry_deferred_event(&mut Context::default());
        assert!(!capacity.deferred_event_open);
        assert!(
            capacity.has_pending(|pending| { matches!(pending, Pending::EventRateWait { .. }) })
        );
    }

    #[test]
    fn account_rate_limit_clears_recovery_gates_instead_of_locking_ui() {
        let mut app = app_with_game(&[], Color::White);
        app.route = Route::Challenge;
        app.accepted_challenge = Some(Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
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
        app.pending_move = Some(super::PendingMove {
            game_id: "abcdEF12".to_owned(),
            movement: "e2e4".to_owned(),
            at_ply: 0,
        });
        app.handle_completed(
            &mut Context::default(),
            Pending::Account,
            b"COBALT-HTTP/1 429\nRetry-After: 15\n\n",
        );
        assert!(app.accepted_challenge.is_none());
        assert!(app.pending_move.is_none());
        assert_eq!(app.route, Route::Play);
        assert!(matches!(app.account, AccountState::Failed(_)));
    }

    #[test]
    fn seek_cancellation_arriving_after_game_start_does_not_stop_game_clock() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.clock.start(&mut context);
        assert!(app.clock.is_running());
        app.handle_cancelled(
            &mut context,
            &Pending::Seek {
                generation: 0,
                preset: api::SeekPreset::Rapid10_0,
            },
        );
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
        app.start_seek(&mut context, api::SeekPreset::Rapid10_0);
        let seek = app.seek_task.expect("seek");
        app.on_action(&mut context, ActionId::BACK);
        assert_eq!(app.route, Route::Pairing);
        assert!(!app.seek_waiting);
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Cancel(task) if *task == seek)));
        app.handle_cancelled(
            &mut context,
            &Pending::Seek {
                generation: 1,
                preset: api::SeekPreset::Rapid10_0,
            },
        );
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
        app.start_seek(&mut context, api::SeekPreset::Rapid10_0);
        app.handle_failed(
            &mut context,
            Pending::Seek {
                generation: 1,
                preset: api::SeekPreset::Rapid10_0,
            },
            kobo_sdk::TaskError::Unreachable,
        );
        assert!(app.seek_waiting);
        assert_eq!(app.route, Route::Pairing);
        let started = api::parse_event(
            br#"{"type":"gameStart","game":{"gameId":"abcdEF12","color":"white","rated":true,"speed":"rapid","source":"lobby","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"Other"}}}"#,
        )
        .expect("matched game");
        app.handle_event(&mut context, started);
        assert!(app.seek_waiting);
        assert!(app.seek_candidate.is_some());
        assert_eq!(app.route, Route::Pairing);
        app.on_action(&mut context, action_id("open-seek-candidate"));
        assert!(!app.seek_waiting);
        assert_eq!(app.route, Route::Game);
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("abcdEF12")
        );
    }

    #[test]
    fn ended_seek_reconciliation_offers_a_matching_new_game() {
        let mut app = ready_app();
        app.summaries = vec![summary("oldGame1", api::SeekPreset::Rapid10_5)];
        let mut context = Context::default();
        app.start_seek(&mut context, api::SeekPreset::Rapid10_5);
        let seek = app.seek_task.expect("seek");
        app.on_task(&mut context, seek, TaskOutcome::Completed(Vec::new()));
        let grace = app
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekGrace { generation: 1 }).then_some(*task)
            })
            .expect("grace");
        app.on_task(&mut context, grace, TaskOutcome::Completed(Vec::new()));
        let reconcile = app
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekReconcile { generation: 1 }).then_some(*task)
            })
            .expect("reconciliation");
        app.on_task(
            &mut context,
            reconcile,
            TaskOutcome::Completed(
                br#"{"nowPlaying":[{"gameId":"oldGame1","color":"white","rated":true,"source":"lobby","speed":"rapid","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"Existing"}},{"gameId":"newGame2","color":"black","rated":true,"source":"lobby","speed":"rapid","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":false,"lastMove":"","opponent":{"username":"NewOpponent"}}]}"#.to_vec(),
            ),
        );
        assert_eq!(app.route, Route::Pairing);
        assert!(app.seek_waiting);
        assert_eq!(app.selected_preset, Some(api::SeekPreset::Rapid10_5));
        assert_eq!(
            app.seek_candidate.as_ref().map(|game| game.id.as_str()),
            Some("newGame2")
        );
        assert!(app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("current games"));
    }

    #[test]
    fn ended_seek_reports_missing_only_after_current_games_are_checked() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_seek(&mut context, api::SeekPreset::Classical30_20);
        let seek = app.seek_task.expect("seek");
        app.on_task(
            &mut context,
            seek,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert!(app.seek_waiting);
        assert!(!app
            .notice
            .as_deref()
            .unwrap_or_default()
            .starts_with("No new rated"));
        let grace = app
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekGrace { generation: 1 }).then_some(*task)
            })
            .expect("grace");
        app.on_task(&mut context, grace, TaskOutcome::Completed(Vec::new()));
        let reconcile = app
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekReconcile { generation: 1 }).then_some(*task)
            })
            .expect("reconciliation");
        app.on_task(
            &mut context,
            reconcile,
            TaskOutcome::Completed(br#"{"nowPlaying":[]}"#.to_vec()),
        );
        assert_eq!(app.route, Route::Play);
        assert!(!app.seek_waiting);
        assert!(app.selected_preset.is_none());
        assert_eq!(app.notice.as_deref(), Some("No match."));
    }

    #[test]
    fn cancelled_ended_seek_cannot_be_reconciled_or_replayed() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_seek(&mut context, api::SeekPreset::Rapid10_5);
        let seek = app.seek_task.expect("seek");
        app.on_task(&mut context, seek, TaskOutcome::Completed(Vec::new()));
        let grace = app
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekGrace { generation: 1 }).then_some(*task)
            })
            .expect("grace");
        app.on_task(&mut context, grace, TaskOutcome::Completed(Vec::new()));
        let reconcile = app
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekReconcile { generation: 1 }).then_some(*task)
            })
            .expect("reconciliation");
        app.cancel_seek(&mut context);
        assert_eq!(app.route, Route::Play);
        app.on_task(
            &mut context,
            reconcile,
            TaskOutcome::Completed(
                br#"{"nowPlaying":[{"gameId":"lateGame","color":"white","rated":true,"source":"lobby","speed":"blitz","variant":{"key":"standard"},"secondsLeft":180,"isMyTurn":true,"lastMove":"","opponent":{"username":"Late"}}]}"#.to_vec(),
            ),
        );
        assert!(app.seek_candidate.is_none());
        assert!(app.session.is_none());
        let seeks = context
            .commands()
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    Command::Spawn {
                        work: kobo_sdk::Task::Post { url, .. },
                        ..
                    } if url.ends_with("/api/board/seek")
                )
            })
            .count();
        assert_eq!(seeks, 1);
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
    fn stale_stream_cancellation_after_resume_cannot_clear_new_stream_state() {
        let mut app = Lichess {
            event_open: true,
            board_open: Some("abcdEF12".to_owned()),
            ..Lichess::default()
        };
        let event_task = kobo_sdk::TaskId(41);
        let board_task = kobo_sdk::TaskId(42);
        app.tasks.insert(event_task, Pending::EventNext);
        app.tasks
            .insert(board_task, Pending::BoardNext("abcdEF12".to_owned()));
        let mut context = Context::default();
        app.close_live_reads(&mut context);
        assert!(!app.tasks.contains_key(&event_task));
        assert!(!app.tasks.contains_key(&board_task));

        app.event_open = true;
        app.board_open = Some("abcdEF12".to_owned());
        app.on_task(&mut context, event_task, TaskOutcome::Cancelled);
        app.on_task(&mut context, board_task, TaskOutcome::Cancelled);
        assert!(app.event_open);
        assert_eq!(app.board_open.as_deref(), Some("abcdEF12"));
    }

    #[test]
    fn suspended_playing_result_cannot_consume_accepted_challenge_recovery() {
        let challenge = Challenge {
            id: "chall123".to_owned(),
            challenger: "ReaderTwo".to_owned(),
            direction: ChallengeDirection::Incoming,
            status: "created".to_owned(),
            rated: false,
            variant: "standard".to_owned(),
            speed: "rapid".to_owned(),
            time_control: ChallengeTime::Clock {
                initial_seconds: Some(600),
                increment_seconds: Some(0),
            },
        };
        let mut app = Lichess {
            accepted_challenge: Some(challenge),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        let playing = kobo_sdk::TaskId(43);
        app.tasks.insert(playing, Pending::Playing);
        let mut context = Context::default();
        app.on_suspend(&mut context);
        assert!(!app.tasks.contains_key(&playing));
        app.on_task(
            &mut context,
            playing,
            TaskOutcome::Completed(
                br#"{"nowPlaying":[{"gameId":"match123","color":"white","rated":false,"source":"friend","speed":"rapid","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"ReaderTwo"}}]}"#
                    .to_vec(),
            ),
        );
        assert!(app.accepted_challenge.is_some());
        assert!(app.reconcile_accepted_challenge);
    }

    #[test]
    fn retired_task_completion_retries_deferred_challenge_reconciliation() {
        let mut app = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            accepted_challenge: Some(Challenge {
                id: "chall123".to_owned(),
                challenger: "ReaderTwo".to_owned(),
                direction: ChallengeDirection::Incoming,
                status: "created".to_owned(),
                rated: false,
                variant: "standard".to_owned(),
                speed: "rapid".to_owned(),
                time_control: ChallengeTime::Clock {
                    initial_seconds: Some(600),
                    increment_seconds: Some(0),
                },
            }),
            reconcile_accepted_challenge: true,
            ..Lichess::default()
        };
        let retired = kobo_sdk::TaskId(88);
        app.retired_tasks.insert(retired, Pending::Playing);
        app.on_task(&mut Context::default(), retired, TaskOutcome::Cancelled);
        assert!(app.has_pending(|pending| matches!(pending, Pending::Playing)));
    }

    #[test]
    fn retired_task_completion_retries_deferred_account_validation() {
        let mut app = Lichess::default();
        let retired = kobo_sdk::TaskId(89);
        app.retired_tasks.insert(retired, Pending::Account);
        app.on_task(&mut Context::default(), retired, TaskOutcome::Cancelled);
        assert!(matches!(app.account, AccountState::Checking));
        assert!(app.has_pending(|pending| matches!(pending, Pending::Account)));
    }

    #[test]
    fn event_reopen_waits_for_older_close_to_settle() {
        let mut app = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            ..Lichess::default()
        };
        app.tasks.insert(kobo_sdk::TaskId(90), Pending::EventClose);
        let mut context = Context::default();
        app.open_event_stream(&mut context);
        assert!(!app.has_pending(|pending| matches!(pending, Pending::EventOpen)));
        app.handle_completed(&mut context, Pending::EventClose, &[]);
        assert!(app.has_pending(|pending| matches!(pending, Pending::EventRetry)));
    }

    #[test]
    fn stream_next_intents_retry_after_task_capacity_clears() {
        let mut full = Context::default();
        for _ in 0..4 {
            assert!(full.spawn(kobo_sdk::Task::Sleep { seconds: 30 }).is_some());
        }
        let mut app = Lichess {
            account: AccountState::Ready(super::Account {
                id: "owner123".to_owned(),
                username: "Owner".to_owned(),
            }),
            event_open: true,
            ..Lichess::default()
        };
        app.next_event(&mut full);
        assert!(app.deferred_event_next);
        app.retry_deferred_event(&mut Context::default());
        assert!(!app.deferred_event_next);
        assert!(app.has_pending(|pending| matches!(pending, Pending::EventNext)));

        let mut board = app_with_game(&[], Color::White);
        board.next_board(&mut full, "abcdEF12");
        assert_eq!(board.deferred_board_next.as_deref(), Some("abcdEF12"));
        board.retry_deferred_board(&mut Context::default());
        assert!(board.deferred_board_next.is_none());
        assert!(board.has_pending(|pending| {
            matches!(pending, Pending::BoardNext(id) if id == "abcdEF12")
        }));
    }

    #[test]
    fn suspend_does_not_cancel_close_tasks_before_they_drop_old_streams() {
        let mut app = Lichess::default();
        let event_close = kobo_sdk::TaskId(91);
        let board_close = kobo_sdk::TaskId(92);
        app.tasks.insert(event_close, Pending::EventClose);
        app.tasks
            .insert(board_close, Pending::BoardClose("abcdEF12".to_owned()));
        let mut context = Context::default();
        app.on_suspend(&mut context);
        assert!(app.tasks.contains_key(&event_close));
        assert!(app.tasks.contains_key(&board_close));
        assert!(!context.commands().iter().any(|command| {
            matches!(
                command,
                Command::Cancel(task) if *task == event_close || *task == board_close
            )
        }));
    }

    #[test]
    fn suspend_defers_closing_idle_retained_streams_when_capacity_is_full() {
        let mut app = app_with_game(&[], Color::White);
        app.event_open = true;
        app.deferred_event_next = true;
        app.deferred_board_next = Some("abcdEF12".to_owned());
        let mut full = Context::default();
        for _ in 0..4 {
            assert!(full.spawn(kobo_sdk::Task::Sleep { seconds: 30 }).is_some());
        }
        app.close_live_reads(&mut full);
        assert!(app.deferred_event_close);
        assert!(app.deferred_board_closes.contains("abcdEF12"));

        let mut available = Context::default();
        app.flush_deferred_stream_closes(&mut available);
        assert!(!app.deferred_event_close);
        assert!(!app.deferred_board_closes.contains("abcdEF12"));
        assert!(app.has_pending(|pending| matches!(pending, Pending::EventClose)));
        assert!(app.has_pending(|pending| {
            matches!(pending, Pending::BoardClose(id) if id == "abcdEF12")
        }));
    }

    #[test]
    fn late_old_game_result_cannot_pause_the_new_board() {
        let mut app = app_with_game(&[], Color::White);
        app.game.as_mut().expect("game").id = "other123".to_owned();
        app.session = Some(Session {
            game_id: "other123".to_owned(),
            color: Color::White,
            opponent: "New opponent".to_owned(),
            rated: true,
        });
        app.board_open = Some("other123".to_owned());
        app.board_ready = true;
        let mut context = Context::default();
        app.handle_completed(
            &mut context,
            Pending::BoardNext("abcdEF12".to_owned()),
            br#"{"type":"gameState","moves":"","wtime":600000,"btime":600000,"status":"started"}"#,
        );
        assert_eq!(app.board_open.as_deref(), Some("other123"));
        assert!(app.board_ready);
        assert_eq!(app.game.as_ref().expect("game").id, "other123");
    }

    #[test]
    fn switching_from_paused_board_retires_old_retry_tasks() {
        let mut app = app_with_game(&[], Color::White);
        app.board_open = None;
        app.board_ready = false;
        app.board_rate_limits
            .insert("abcdEF12".to_owned(), super::unix_seconds() + 30);
        let old_wait = kobo_sdk::TaskId(77);
        app.tasks.insert(
            old_wait,
            Pending::BoardRateWait {
                id: "abcdEF12".to_owned(),
                remaining: 30,
            },
        );
        let mut context = Context::default();
        app.open_board(
            &mut context,
            Session {
                game_id: "other123".to_owned(),
                color: Color::Black,
                opponent: "New opponent".to_owned(),
                rated: true,
            },
        );
        assert!(!app.tasks.contains_key(&old_wait));
        assert!(app.game.is_none());
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("other123")
        );
        assert!(app.has_pending(|pending| {
            matches!(pending, Pending::BoardOpen(id) if id == "other123")
        }));
        assert!(app.board_rate_limits.contains_key("abcdEF12"));
    }

    #[test]
    fn board_open_intent_retries_after_task_capacity_clears() {
        let mut app = Lichess::default();
        let session = Session {
            game_id: "abcdEF12".to_owned(),
            color: Color::White,
            opponent: "Other".to_owned(),
            rated: true,
        };
        let mut full = Context::default();
        for _ in 0..4 {
            assert!(full.spawn(kobo_sdk::Task::Sleep { seconds: 30 }).is_some());
        }
        app.open_board(&mut full, session.clone());
        assert_eq!(app.deferred_board_open, Some(session));
        assert_eq!(app.route, Route::Game);
        assert!(!app.has_pending(|pending| matches!(pending, Pending::BoardOpen(_))));

        let mut available = Context::default();
        app.retry_deferred_board(&mut available);
        assert!(app.deferred_board_open.is_none());
        assert!(app.has_pending(|pending| {
            matches!(pending, Pending::BoardOpen(id) if id == "abcdEF12")
        }));
    }

    #[test]
    fn stale_playing_snapshot_clears_the_in_memory_board_and_route() {
        let mut app = app_with_game(&[], Color::White);
        app.board_open = Some("abcdEF12".to_owned());
        let mut context = Context::default();
        app.clock.start(&mut context);
        app.pending_action = Some(GameAction::Resign);
        app.handle_completed(&mut context, Pending::Playing, br#"{"nowPlaying":[]}"#);
        assert!(app.session.is_none());
        assert!(app.game.is_none());
        assert!(app.board_open.is_none());
        assert!(app.pending_action.is_none());
        assert!(!app.clock.is_running());
        assert_eq!(app.route, Route::Play);
    }

    #[test]
    fn board_rate_limit_disables_actions_and_waits_retry_after() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.clock.start(&mut context);
        app.handle_completed(
            &mut context,
            Pending::BoardNext("abcdEF12".to_owned()),
            b"COBALT-HTTP/1 429\nRetry-After: 23\n\n",
        );
        assert!(app.game.is_some());
        assert!(!app.board_ready);
        assert!(app.board_open.is_none());
        assert!(!app.clock.is_running());
        assert!(app.has_pending(|pending| {
            matches!(
                pending,
                Pending::BoardRateWait {
                    id,
                    remaining: 0
                } if id == "abcdEF12"
            )
        }));
        let screen = format!("{:?}", app.game_screen());
        assert!(screen.contains("Reconnect board"));
        assert!(!screen.contains("Offer draw"));
        app.set_board_rate_limit(&mut context, "other123", 31);
        let saved = context
            .commands()
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Store(StoreRequest::Save { key, value }) if key == BOARD_RATE_KEY => {
                    super::decode_board_rate_limits(value)
                }
                _ => None,
            });
        let saved = saved.expect("persisted retry deadline");
        assert!(saved
            .get("abcdEF12")
            .is_some_and(|not_before| *not_before >= super::unix_seconds()));
        assert!(saved
            .get("other123")
            .is_some_and(|not_before| *not_before >= super::unix_seconds()));

        app.close_live_reads(&mut context);
        assert!(!app.has_pending(|pending| { matches!(pending, Pending::BoardRateWait { .. }) }));
        let mut resumed = app_with_game(&[], Color::White);
        resumed.board_open = None;
        resumed.board_ready = false;
        resumed.board_rate_limits = saved;
        let mut resumed_context = Context::default();
        let session = resumed.session.clone().expect("session");
        resumed.open_board(&mut resumed_context, session);
        assert!(resumed.has_pending(|pending| { matches!(pending, Pending::BoardRateWait { .. }) }));
        assert!(!resumed.has_pending(|pending| { matches!(pending, Pending::BoardOpen(_)) }));
    }

    #[test]
    fn unresolved_game_action_blocks_navigation_and_board_switching() {
        let mut app = app_with_game(&[], Color::White);
        app.pending_action = Some(GameAction::Resign);
        let mut context = Context::default();
        app.on_action(&mut context, ActionId::BACK);
        assert_eq!(app.route, Route::Game);
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("abcdEF12")
        );

        app.open_board(
            &mut context,
            Session {
                game_id: "other123".to_owned(),
                color: Color::Black,
                opponent: "Someone".to_owned(),
                rated: true,
            },
        );
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("abcdEF12")
        );
    }

    #[test]
    fn late_action_generation_cannot_clear_a_new_identical_action() {
        let mut app = app_with_game(&[], Color::White);
        app.pending_action = Some(GameAction::OfferDraw);
        app.pending_scope = Some(ActionScope::Game("abcdEF12".to_owned()));
        app.pending_action_generation = Some(2);
        let mut context = Context::default();
        app.handle_completed(
            &mut context,
            Pending::Action {
                action: GameAction::OfferDraw,
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 1,
            },
            &[],
        );
        assert_eq!(app.pending_action, Some(GameAction::OfferDraw));
        assert_eq!(app.pending_action_generation, Some(2));
    }

    #[test]
    fn unsupported_game_full_stops_instead_of_retrying_forever() {
        let mut app = Lichess {
            route: Route::Game,
            session: Some(Session {
                game_id: "abcdEF12".to_owned(),
                color: Color::White,
                opponent: "Other".to_owned(),
                rated: false,
            }),
            board_open: Some("abcdEF12".to_owned()),
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.handle_board(
            &mut context,
            "abcdEF12",
            api::BoardRecord::Unsupported("chess960".to_owned()),
        );
        assert!(app.session.is_none());
        assert!(app.game.is_none());
        assert_eq!(app.route, Route::Play);
        assert!(!app.has_pending(|pending| matches!(pending, Pending::BoardRetry(_))));
    }

    #[test]
    fn game_finish_on_paused_board_clears_resumable_state() {
        let mut app = app_with_game(&[], Color::White);
        app.board_open = None;
        app.board_ready = false;
        let mut context = Context::default();
        app.handle_event(&mut context, api::Event::GameFinish("abcdEF12".to_owned()));
        assert!(app.session.is_none());
        assert!(app.game.is_none());
        assert_eq!(app.route, Route::Play);
        assert!(!format!("{:?}", app.play_screen()).contains("Current board"));
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
        assert!(format!("{:?}", app.play_screen()).contains("Offline"));
        app.tasks.insert(kobo_sdk::TaskId(8), Pending::Account);
        app.on_task(
            &mut context,
            kobo_sdk::TaskId(8),
            TaskOutcome::Completed(b"COBALT-HTTP/1 429\nRetry-After: 17\n\n".to_vec()),
        );
        assert!(app.notice.as_deref().unwrap_or_default().contains("17s"));
    }

    #[test]
    fn fresh_start_validates_an_account_without_a_saved_game() {
        let mut app = Lichess {
            loaded_session: true,
            loaded_puzzles: true,
            loaded_board_rate: true,
            loaded_event_rate: true,
            loaded_seek_rate: true,
            ..Lichess::default()
        };
        let mut context = Context::default();

        app.maybe_start(&mut context);

        assert!(matches!(app.account, AccountState::Checking));
        assert!(app.has_pending(|pending| matches!(pending, Pending::Account)));
    }

    #[test]
    fn missing_credential_is_rechecked_while_the_app_remains_open() {
        let mut app = Lichess::default();
        let mut context = Context::default();
        let account = kobo_sdk::TaskId(7);
        app.tasks.insert(account, Pending::Account);

        app.on_task(
            &mut context,
            account,
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
        );

        assert!(matches!(app.account, AccountState::Missing));
        let retry = app
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::AccountRetry).then_some(*task))
            .expect("credential retry");
        assert!(context.commands().iter().any(|command| {
            matches!(
                command,
                Command::Spawn {
                    work: kobo_sdk::Task::Sleep {
                        seconds: ACCOUNT_RETRY_SECONDS
                    },
                    ..
                }
            )
        }));

        app.on_task(&mut context, retry, TaskOutcome::Completed(Vec::new()));

        assert!(matches!(app.account, AccountState::Checking));
        assert!(app.has_pending(|pending| matches!(pending, Pending::Account)));
    }

    #[test]
    fn post_opening_actions_hide_abort_and_keep_resign_and_draw_controls() {
        let mut app = app_with_game(&["e2e4", "e7e5"], Color::White);
        app.menu_open = true;
        let rendered = format!("{:?}", app.game_screen());
        assert!(!rendered.contains("Abort"));
        assert!(rendered.contains("Resign"));
        assert!(rendered.contains("Offer draw"));
    }

    #[test]
    fn draw_acceptance_and_decline_wait_for_authoritative_server_state() {
        let mut accepting = app_with_game(&["e2e4", "e7e5"], Color::White);
        accepting.game.as_mut().expect("game").state.black_draw = true;
        let offered = format!("{:?}", accepting.game_screen());
        assert!(offered.contains("Accept draw"));
        assert!(offered.contains("Decline draw"));
        let mut accept_context = Context::default();
        accepting.on_action(&mut accept_context, action_id("accept-draw"));
        accepting.handle_completed(
            &mut accept_context,
            Pending::Action {
                action: GameAction::AcceptDraw,
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 1,
            },
            &[],
        );
        assert!(
            accepting
                .game
                .as_ref()
                .expect("game")
                .draw_offer_from_opponent(),
            "a successful POST must not invent local draw acceptance"
        );
        assert!(format!("{:?}", accepting.game_screen())
            .contains("Lichess accepted the request; waiting for the stream"));
        let accepted = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"draw"}"#,
            "abcdEF12",
        )
        .expect("accepted draw");
        accepting.handle_board(&mut accept_context, "abcdEF12", accepted);
        assert!(format!("{:?}", accepting.game_screen()).contains("Draw agreed"));

        let mut declining = app_with_game(&["e2e4", "e7e5"], Color::White);
        declining.game.as_mut().expect("game").state.black_draw = true;
        let mut decline_context = Context::default();
        declining.on_action(&mut decline_context, action_id("decline-draw"));
        declining.handle_completed(
            &mut decline_context,
            Pending::Action {
                action: GameAction::DeclineDraw,
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 1,
            },
            &[],
        );
        assert!(
            declining
                .game
                .as_ref()
                .expect("game")
                .draw_offer_from_opponent(),
            "a successful POST must not invent local draw decline"
        );
        let declined = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"started","bdraw":false}"#,
            "abcdEF12",
        )
        .expect("declined draw");
        declining.handle_board(&mut decline_context, "abcdEF12", declined);
        let cleared = format!("{:?}", declining.game_screen());
        assert!(cleared.contains("Offer draw"));
        assert!(!cleared.contains("Accept draw"));
        assert!(!cleared.contains("Decline draw"));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one fixture follows the complete seek, game, reconnect, move, and draw lifecycle"
    )]
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
        assert_eq!(app.route, Route::Pairing);
        assert!(app.seek_candidate.is_some());
        app.on_action(&mut pairing, action_id("open-seek-candidate"));
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
        app.handle_completed(&mut stream, Pending::BoardOpen("abcdEF12".to_owned()), &[]);
        app.handle_board(&mut stream, "abcdEF12", full);
        assert!(app.game.as_ref().expect("game").state.moves.is_empty());

        let mut move_context = Context::default();
        app.on_action(&mut move_context, action_id("square-e2"));
        app.on_action(&mut move_context, action_id("square-e4"));
        assert!(app.pending_move.is_some());
        app.handle_completed(
            &mut move_context,
            Pending::Action {
                action: GameAction::Move("e2e4".to_owned()),
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 1,
            },
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
        app.handle_completed(&mut stream, Pending::BoardOpen("abcdEF12".to_owned()), &[]);
        app.handle_board(&mut stream, "abcdEF12", reconnected);
        assert!(app.game.as_ref().expect("game").draw_offer_from_opponent());

        let mut draw_context = Context::default();
        app.on_action(&mut draw_context, action_id("accept-draw"));
        assert_eq!(app.pending_action, Some(GameAction::AcceptDraw));
        app.handle_completed(
            &mut draw_context,
            Pending::Action {
                action: GameAction::AcceptDraw,
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 2,
            },
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
        app.route = Route::Play;
        assert!(!format!("{:?}", app.play_screen()).contains("Current board"));
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

        app.clear_pending_action();
        app.accepted_challenge = None;
        app.route = Route::Game;
        app.on_action(&mut context, action_id("confirm-resign"));
        app.on_action(&mut context, action_id("resign"));
        assert_eq!(app.pending_action, Some(GameAction::Resign));
        app.handle_completed(
            &mut context,
            Pending::Action {
                action: GameAction::Resign,
                scope: ActionScope::Game("abcdEF12".to_owned()),
                generation: 2,
            },
            &[],
        );
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
