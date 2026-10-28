//! An unofficial Lichess Board API client for Cobalt.
//!
//! The application names the `lichess` secret but never receives its value.
//! Live HTTP streams, redirects, cancellation, and credential attachment stay
//! in the runtime. An owner without a computer can type the token on the
//! reader: the app holds those keystrokes only until it hands them to the
//! runtime, and cannot read the stored token back.

mod api;
mod chess;
mod model;

use api::{BoardRecord, Event, SeekPreset};
use kobo_json::{ObjectBuilder, Value};
use kobo_sdk::credentials::{CredentialEvent, CredentialSetup};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BandAlign, BannerLevel, Context, ControlState, DeviceRequest,
    DeviceResult, Failure, Glyph, Heartbeat, KoboApp, Orientation, Screen, ScreenBuilder,
    SlotWidth, StoreResult, Task, TaskError, TaskId, TaskOutcome, Tile, TileShape, TileState,
};
#[cfg(any(test, debug_assertions))]
use model::ChallengeTime;
use model::{
    Account, ApplyState, Challenge, ChallengeDirection, Color, FullGame, Game, GameSummary, Player,
    ServerState, Session,
};
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const SESSION_KEY: &str = "lichess.session.v1";
const PUZZLE_KEY: &str = "lichess.puzzles.v1";
const BOARD_RATE_KEY: &str = "lichess.board-rate.v1";
const EVENT_RATE_KEY: &str = "lichess.event-rate.v1";
const SEEK_RATE_KEY: &str = "lichess.seek-rate.v1";
/// The owner's standing choices: rated or casual seeks, and the side and
/// clock for games against the computer. Kept apart from the session so a
/// corrupt preference can be dropped without touching a game in progress.
const PREFERENCES_KEY: &str = "lichess.preferences.v1";
const MAX_STORED_PUZZLES: usize = 32;
const ACCOUNT_RETRY_SECONDS: u32 = 15;
const CONNECT: &str = "connect-lichess";
/// Where the token comes from, said on the reader that asks for it.
///
/// The website's setup steps link to a form with the scope already ticked,
/// but someone who installed from the Store on the reader may never have
/// seen them. The address is short enough to type into a phone, and naming
/// the one scope keeps the token from granting more than the Board API needs.
const TOKEN_GUIDANCE: &str = "On a phone or computer, create a token at \
lichess.org/account/oauth/token with only board:play. Enter it here as the account key.";
const HOME_TILE_COUNT: usize = SeekPreset::ALL.len() + 4;

struct HomeTile {
    action: String,
    label: String,
    glyph: Glyph,
    subtitle: String,
    state: TileState,
}

type BuilderSlot = (SlotWidth, Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Home,
    HowTo,
    Puzzles,
    Solve,
    PuzzleResult,
    Play,
    Pairing,
    ChallengePlayer,
    ChallengeSetup,
    Challenge,
    ComputerSetup,
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
    CreateChallenge {
        username: String,
    },
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ChallengeSide {
    Black,
    #[default]
    Random,
    White,
}

impl ChallengeSide {
    const ALL: [Self; 3] = [Self::Black, Self::Random, Self::White];

    const fn action(self) -> &'static str {
        match self {
            Self::Black => "challenge-side-black",
            Self::Random => "challenge-side-random",
            Self::White => "challenge-side-white",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Black => "Black",
            Self::Random => "Random",
            Self::White => "White",
        }
    }

    const fn api_value(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Random => "random",
            Self::White => "white",
        }
    }

    const fn computer_action(self) -> &'static str {
        match self {
            Self::Black => "computer-side-black",
            Self::Random => "computer-side-random",
            Self::White => "computer-side-white",
        }
    }

    fn from_api_value(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|side| side.api_value() == value)
    }

    /// The colour the owner plays. Random is settled once, when the game
    /// starts, from the clock's spare nanoseconds: the computer is no
    /// opponent to cheat, so any coin that lands both ways often enough will do.
    fn resolve(self) -> Color {
        match self {
            Self::White => Color::White,
            Self::Black => Color::Black,
            Self::Random => {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |duration| duration.subsec_nanos());
                if nanos / 1000 % 2 == 0 {
                    Color::White
                } else {
                    Color::Black
                }
            }
        }
    }
}

/// How long each side has in a game against the computer.
///
/// No clock comes first and is the default because the computer answers at
/// once and never runs short, so a clock only ever presses the owner, and an
/// unclocked board is also one the panel never repaints between moves.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ComputerClock {
    #[default]
    Untimed,
    Ten,
    Thirty,
}

impl ComputerClock {
    const ALL: [Self; 3] = [Self::Untimed, Self::Ten, Self::Thirty];

    const fn action(self) -> &'static str {
        match self {
            Self::Untimed => "computer-clock-none",
            Self::Ten => "computer-clock-10",
            Self::Thirty => "computer-clock-30",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Untimed => "No clock",
            Self::Ten => "10 min",
            Self::Thirty => "30 min",
        }
    }

    const fn stored(self) -> &'static str {
        match self {
            Self::Untimed => "none",
            Self::Ten => "10",
            Self::Thirty => "30",
        }
    }

    fn from_stored(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|clock| clock.stored() == value)
    }

    const fn milliseconds(self) -> Option<u64> {
        match self {
            Self::Untimed => None,
            Self::Ten => Some(600_000),
            Self::Thirty => Some(1_800_000),
        }
    }
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
    challenge_keyboard: Keyboard,
    challenge_username: String,
    challenge_preset: SeekPreset,
    challenge_side: ChallengeSide,
    /// Whether a time tile seeks a rated game. The owner's standing choice,
    /// made in Account/Games, named on every time tile and kept across
    /// restarts.
    seek_rated: bool,
    /// Whether the seek in flight is rated. Captured when the seek starts so
    /// that changing the choice while a seek waits cannot make the app look
    /// for a game from the other pool.
    selected_rated: bool,
    computer_side: ChallengeSide,
    computer_clock: ComputerClock,
    local_game: bool,
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
    expected_seek_game: Option<(String, SeekPreset, bool)>,
    seek_rate_limit: Option<u64>,
    pending_action: Option<GameAction>,
    pending_scope: Option<ActionScope>,
    pending_action_generation: Option<u64>,
    next_action_generation: u64,
    accepted_challenge: Option<Challenge>,
    reconcile_accepted_challenge: bool,
    pending_move: Option<PendingMove>,
    selected: Option<String>,
    invalid_square: Option<String>,
    invalid_until_tick: u64,
    promotion: Option<Promotion>,
    confirmation: Option<Confirmation>,
    menu_open: bool,
    result_dismissed: bool,
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
    credential: CredentialSetup,
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
            challenge_keyboard: Keyboard::new(),
            challenge_username: String::new(),
            challenge_preset: SeekPreset::Rapid10_0,
            challenge_side: ChallengeSide::Random,
            seek_rated: true,
            selected_rated: true,
            computer_side: ChallengeSide::White,
            computer_clock: ComputerClock::Untimed,
            local_game: false,
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
            invalid_square: None,
            invalid_until_tick: 0,
            promotion: None,
            confirmation: None,
            menu_open: false,
            result_dismissed: false,
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
            credential: CredentialSetup::new(api::SECRET, "Lichess").with_guidance(TOKEN_GUIDANCE),
        }
    }
}

impl Lichess {
    fn show(&mut self, context: &mut Context) {
        if self.credential.is_open() {
            // Back closes the prompt onto Account/Games rather than leaving
            // the app, which is where the owner opened it from.
            context.set_screen(self.credential.screen("Lichess").with_own_back(true));
            return;
        }
        let screen = match self.route {
            Route::Home => self.home(context),
            Route::HowTo => Self::how_to_screen(),
            Route::Puzzles => self.puzzles_screen(),
            Route::Solve => self.solve_screen(),
            Route::PuzzleResult => self.puzzle_result(),
            Route::Play => self.play_screen(),
            Route::Pairing => self.pairing_screen(),
            Route::ChallengePlayer => self.challenge_player_screen(),
            Route::ChallengeSetup => self.challenge_setup_screen(),
            Route::Challenge => self.challenge_screen(),
            Route::ComputerSetup => self.computer_setup_screen(),
            Route::Game => self.game_screen(context),
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
            AccountState::Unknown => "Offline".to_owned(),
            AccountState::Missing => "Not connected".to_owned(),
            AccountState::Invalid => "Token rejected".to_owned(),
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

    /// How a time tile looks while a seek cannot start yet.
    ///
    /// Never unavailable. A greyed tile does not answer a tap, and most of the
    /// reasons a seek cannot start are ones a tap can resolve or at least
    /// name, so the tile stays live and [`Self::explain_unready_seek`] answers
    /// it. The busy mark appears only while the app is still connecting, the
    /// one case where waiting a moment is the whole fix.
    fn seek_tile_state(&self) -> TileState {
        let connecting = matches!(self.account, AccountState::Checking)
            || (matches!(self.account, AccountState::Ready(_))
                && (!self.event_open || !self.playing_ready))
            || self.seek_task.is_some()
            || self.seek_waiting;
        if !self.seek_ready() && connecting {
            TileState::Busy
        } else {
            TileState::Normal
        }
    }

    /// One home card. A time card names the pool it seeks in, so the owner
    /// knows before tapping whether the game will touch their rating; the
    /// choice itself lives in Account/Games, because a row of chips above
    /// the cards costs the smallest panel a whole row of them.
    fn home_tile(index: usize, seek_state: TileState, rated: bool) -> HomeTile {
        if index == 0 {
            HomeTile {
                action: "play".to_owned(),
                label: "Account/Games".to_owned(),
                glyph: Glyph::Settings,
                subtitle: "Challenges · boards".to_owned(),
                state: TileState::Normal,
            }
        } else if index == 1 {
            HomeTile {
                action: "puzzles".to_owned(),
                label: "Puzzles".to_owned(),
                glyph: Glyph::Grid,
                subtitle: "Offline training".to_owned(),
                state: TileState::Normal,
            }
        } else if index == 2 {
            HomeTile {
                action: "play-computer".to_owned(),
                label: "Computer".to_owned(),
                glyph: Glyph::ChessBlackKnight,
                subtitle: "Offline".to_owned(),
                state: TileState::Normal,
            }
        } else if index == 3 {
            HomeTile {
                action: "how-to-play".to_owned(),
                label: "How to play".to_owned(),
                glyph: Glyph::Note,
                subtitle: "Board · controls".to_owned(),
                state: TileState::Normal,
            }
        } else {
            let preset = SeekPreset::ALL[index - 4];
            HomeTile {
                action: preset.action().to_owned(),
                label: preset.label(),
                glyph: Glyph::Clock,
                subtitle: format!(
                    "{} · {}",
                    preset.speed_label(),
                    if rated { "Rated" } else { "Casual" }
                ),
                state: seek_state,
            }
        }
    }

    fn home_card_row(screen: ScreenBuilder, tiles: Vec<HomeTile>, columns: usize) -> ScreenBuilder {
        let mut slots: Vec<BuilderSlot> = tiles
            .into_iter()
            .map(|tile| {
                (
                    SlotWidth::Fill,
                    Box::new(move |slot: ScreenBuilder| {
                        let state = tile.state;
                        slot.tile_grid(
                            TileShape::Card,
                            [(tile.action, tile.label, tile.glyph, move |card: Tile| {
                                card.with_subtitle(tile.subtitle).with_state(state)
                            })],
                        )
                    }) as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                )
            })
            .collect();
        while slots.len() < columns {
            slots.push((SlotWidth::Fill, Box::new(|slot| slot)));
        }
        screen.band(BandAlign::Top, slots)
    }

    fn home(&mut self, context: &Context) -> Screen {
        let pages = self.home_pages(context);
        self.home_page = self.home_page.min(pages.len().saturating_sub(1));
        let page = self.home_page;
        let page_count = u16::try_from(pages.len()).unwrap_or(u16::MAX);
        let page_index = u16::try_from(page).unwrap_or(u16::MAX);
        let page_number = u16::try_from(page.saturating_add(1)).unwrap_or(u16::MAX);
        let seek_state = self.seek_tile_state();
        let columns = context
            .metrics()
            .grid_columns(TileShape::Square)
            .clamp(1, 3);
        let mut screen = self.home_header().page_rail(page_index, page_count);
        for row in pages[page].chunks(columns) {
            let tiles = row
                .iter()
                .map(|index| Self::home_tile(*index, seek_state, self.seek_rated))
                .collect();
            screen = Self::home_card_row(screen, tiles, columns);
        }
        // A position only where there is somewhere to turn to: "1 of 1" under
        // a home screen that fits on one page reads as a list that failed to
        // load the rest.
        if page_count > 1 {
            screen = screen
                .page_turns("home-previous", "home-next")
                .page_position(page_number, page_count);
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
                        .map_or_else(|| "None".to_owned(), |puzzle| puzzle.id.clone()),
                ),
            ]);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let fetching = self.has_pending(|pending| matches!(pending, Pending::Puzzle));
        screen = screen.button_with_state(
            "new-puzzles",
            if fetching {
                "Downloading…"
            } else {
                "Download 32 puzzles"
            },
            enabled(!fetching),
        );
        // Resume appears only when there is a puzzle to resume; a grey button
        // beside Download only repeated that none had been downloaded yet.
        if self.remaining_puzzles() > 0 {
            screen = screen.button("solve", "Resume puzzle");
        }
        screen.build()
    }

    fn how_to_screen() -> Screen {
        ScreenBuilder::new("lichess-help")
            .top_bar("How to play")
            .heading("Chess on Lichess")
            .text("Tap a piece, then a marked square. Tap the selected piece again to cancel.")
            .text("Outlined squares show the last move.")
            .text("Computer starts an offline game. Choose your side and whether to use a clock.")
            .text("Time tiles seek a live opponent. Choose rated or casual games in Account/Games.")
            .text("Account/Games holds challenges and ongoing boards. Puzzles work offline after download.")
            .text("During a live game, the clock and board update while this screen is open.")
            .bottom_action("home", "Play")
            .build()
    }

    fn solve_screen(&self) -> Screen {
        let Some(puzzle) = self.puzzles.get(self.current_puzzle) else {
            return ScreenBuilder::new("lichess-solve")
                .top_bar("Solve")
                .empty_state("No puzzle is ready. Download puzzles on Wi-Fi.")
                .build();
        };
        let orientation = if chess::side_to_move(&puzzle.fen) == Some('b') {
            Color::Black
        } else {
            Color::White
        };
        let puzzle_status = self.selected.as_deref().map_or_else(
            || orientation.name().to_owned(),
            |square| format!("{} · {square} →", orientation.name()),
        );
        let mut screen = ScreenBuilder::new("lichess-solve")
            .top_bar("Puzzle")
            .secondary(puzzle_status);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let last_move = puzzle
            .cursor
            .checked_sub(1)
            .and_then(|index| puzzle.solution.get(index))
            .map(String::as_str);
        screen
            .board_with_selection(
                8,
                board_cells(
                    &puzzle.fen,
                    orientation,
                    self.selected.as_deref(),
                    self.invalid_square
                        .as_deref()
                        .filter(|_| self.total_ticks < self.invalid_until_tick),
                    last_move,
                ),
            )
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
            AccountState::Unknown => screen.secondary("Offline"),
            AccountState::Missing => screen.secondary("Not connected").text(
                "Connect a Lichess account to play people. Computer and Puzzles work without one.",
            ),
            AccountState::Checking => screen.activity("Checking account", None),
            AccountState::Ready(account) => screen.secondary(account.username.clone()),
            AccountState::Invalid => screen.banner(BannerLevel::Attention, "Token rejected"),
            AccountState::Failed(message) => screen.banner(BannerLevel::Attention, message),
        };
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if matches!(self.account, AccountState::Missing | AccountState::Invalid) {
            screen = screen.primary_button(CONNECT, "Connect Lichess");
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
        screen = screen.section("Time tiles").chips([
            ("seek-rated", "Rated", self.seek_rated),
            ("seek-casual", "Casual", !self.seek_rated),
        ]);
        // Without an account a challenge can never be sent, so the button is
        // left out rather than shown grey under the Connect button that is
        // the actual next step.
        if !matches!(self.account, AccountState::Missing | AccountState::Invalid) {
            screen = screen.button_with_state(
                "challenge-player",
                "Challenge player",
                enabled(matches!(self.account, AccountState::Ready(_))),
            );
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

    fn challenge_player_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-challenge-player")
            .top_bar("Challenge player")
            .typed(&self.challenge_keyboard, "Lichess username");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen.keyboard(&self.challenge_keyboard, "Next").build()
    }

    fn challenge_setup_screen(&self) -> Screen {
        let pending =
            self.has_pending(|pending| matches!(pending, Pending::CreateChallenge { .. }));
        let mut screen = ScreenBuilder::new("lichess-challenge-setup")
            .top_bar("Challenge player")
            .heading(self.challenge_username.clone())
            .secondary("Casual")
            .section("Time")
            .chips(SeekPreset::ALL.into_iter().map(|preset| {
                (
                    format!("challenge-time-{}", preset.action()),
                    preset.label(),
                    preset == self.challenge_preset,
                )
            }))
            .section("Side")
            .chips(
                ChallengeSide::ALL
                    .into_iter()
                    .map(|side| (side.action(), side.label(), side == self.challenge_side)),
            );
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .button_with_state(
                "send-player-challenge",
                if pending { "Sending" } else { "Challenge" },
                if pending {
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
        let kind = if self.selected_rated {
            "Rated"
        } else {
            "Casual"
        };
        if let Some(candidate) = &self.seek_candidate {
            return ScreenBuilder::new("lichess-pairing-candidate")
                .top_bar("Game started")
                .heading(format!("vs {}", candidate.opponent))
                .secondary(format!("{clock} {} · {kind}", preset.speed_label()))
                .primary_button("open-seek-candidate", "Open game")
                .buttons([("keep-seeking", "Keep waiting"), ("cancel-seek", "Cancel")])
                .build();
        }
        let reconciling =
            self.has_pending(|pending| matches!(pending, Pending::SeekReconcile { .. }));
        let mut screen = ScreenBuilder::new("lichess-pairing")
            .top_bar("Pairing")
            .heading(clock)
            .secondary(format!("{} · {kind}", preset.speed_label()))
            .activity(
                if reconciling {
                    "Checking games"
                } else if self.seek_task.is_some() {
                    "Finding an opponent"
                } else {
                    "Waiting to check again"
                },
                None,
            )
            .secondary(self.clock.waited_words());
        if let Some(notice) = self
            .notice
            .as_ref()
            .filter(|notice| !reconciling || notice.as_str() != "Checking games.")
        {
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

    fn computer_setup_screen(&self) -> Screen {
        ScreenBuilder::new("lichess-computer-setup")
            .top_bar("Computer")
            .secondary("Offline · Casual")
            .section("Side")
            .chips(ChallengeSide::ALL.into_iter().map(|side| {
                (
                    side.computer_action(),
                    side.label(),
                    side == self.computer_side,
                )
            }))
            .section("Clock")
            .chips(
                ComputerClock::ALL
                    .into_iter()
                    .map(|clock| (clock.action(), clock.label(), clock == self.computer_clock)),
            )
            .primary_button("start-computer", "Start game")
            .build()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the board, clocks, result, promotion, confirmation, and pending overlays form one screen"
    )]
    fn game_screen(&self, context: &Context) -> Screen {
        let Some(game) = &self.game else {
            return ScreenBuilder::new("lichess-game")
                .top_bar("Game")
                .activity("Opening your game", None)
                .build();
        };
        let elapsed = self.clock.waited().as_secs();
        let clocks_confirmed = self.board_is_live(&game.id) || !game.active();
        let displayed_clock = |color| {
            if clocks_confirmed {
                clock(game.clock_ms(color, elapsed))
            } else {
                "--:--".to_owned()
            }
        };
        let white = displayed_clock(Color::White);
        let black = displayed_clock(Color::Black);
        let (you_name, you_color, you_clock, opponent_name, opponent_color, opponent_clock) =
            match game.my_color {
                Color::White => (
                    game.white.name.clone(),
                    "White",
                    white,
                    game.black.name.clone(),
                    "Black",
                    black,
                ),
                Color::Black => (
                    game.black.name.clone(),
                    "Black",
                    black,
                    game.white.name.clone(),
                    "White",
                    white,
                ),
            };
        let metrics = context.metrics();
        let name_width = metrics.prose_area(true, false).width
            - metrics.tenth_mm(220)
            - metrics.space(kobo_ui::Space::Small);
        let player_name = |name: String| {
            kobo_ui::with_text_scale(metrics.text_scale, || {
                kobo_ui::clamp_lines(&name, name_width.max(1), kobo_ui::FontSize::Body, 1)
            })
        };
        let you_name = player_name(you_name);
        let opponent_name = player_name(opponent_name);
        let last = game
            .last_san
            .as_deref()
            .or_else(|| game.state.moves.last().map(String::as_str))
            .unwrap_or("none");
        let sent = self
            .pending_move
            .as_ref()
            .filter(|pending| pending.game_id == game.id);
        let move_status = match (self.selected.as_deref(), sent) {
            (Some(square), _) => format!("{square} →"),
            (None, Some(pending)) => pending.movement.clone(),
            (None, None) => last.to_owned(),
        };
        let live = self.board_is_live(&game.id);
        let blocked = !live || self.pending_action.is_some() || self.pending_move.is_some();
        let mut menu = Vec::new();
        if game.active() && !blocked {
            if game.draw_offer_from_opponent() {
                menu.push(("accept-draw".to_owned(), "Accept draw".to_owned()));
                menu.push(("decline-draw".to_owned(), "Decline draw".to_owned()));
            } else {
                menu.push(("offer-draw".to_owned(), "Offer draw".to_owned()));
            }
            if game.opponent_gone && self.claim_remaining() == Some(0) {
                menu.push(("claim-victory".to_owned(), "Claim victory".to_owned()));
            }
            menu.push(("confirm-resign".to_owned(), "Resign".to_owned()));
            if game.can_abort() {
                menu.push(("confirm-abort".to_owned(), "Abort".to_owned()));
            }
        } else if game.active()
            && !live
            && self.pending_action.is_none()
            && self.pending_move.is_none()
        {
            menu.push(("reconnect-board".to_owned(), "Reconnect".to_owned()));
        }
        let turn = if !game.active() {
            "Finished"
        } else if !live {
            "Reconnecting"
        } else if sent.is_some() {
            "Move sent"
        } else if game.my_turn() {
            "Your move"
        } else {
            "Opponent's move"
        };
        let your_active = live && game.active() && game.my_turn();
        let opponent_active = live && game.active() && !game.my_turn();
        let game_kind = if game.rated { "Rated" } else { "Casual" };
        let check = if game.check { " · Check" } else { "" };
        // A game without a clock shows none, rather than a chip that never
        // moves, and nothing on the screen changes between moves. The owner's
        // clock gives way to Undo instead of to nothing: the board is sized to
        // leave room only for a control below it, and with bare text there it
        // grew over the owner's own status line at large text sizes.
        let timed = game.timed;
        let mut opponent_slots: Vec<BuilderSlot> = vec![(
            SlotWidth::Fill,
            Box::new(move |slot| slot.text(opponent_name).secondary(opponent_color)),
        )];
        if timed {
            opponent_slots.push((
                SlotWidth::Fixed(220),
                Box::new(move |slot| {
                    slot.chips([("opponent-clock", opponent_clock, opponent_active)])
                }),
            ));
        }
        let mut screen = ScreenBuilder::new("lichess-game")
            .top_bar("Lichess")
            .top_bar_overflow("game-menu", self.menu_open, menu)
            .band(BandAlign::Top, opponent_slots);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if game.takeback_pending() {
            screen = screen.banner(
                BannerLevel::Attention,
                "Takebacks are unavailable here. The board updates if moves change on Lichess.",
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
        // A sent move is outlined where the last move would be, so the owner
        // sees which move is waiting without a dialog over the board. The
        // pieces stay put until Lichess confirms it, and then only they move.
        let last_move = sent
            .map(|pending| pending.movement.as_str())
            .or_else(|| game.state.moves.last().map(String::as_str));
        let board = board_cells(
            &game.fen,
            game.my_color,
            self.selected.as_deref(),
            self.invalid_square
                .as_deref()
                .filter(|_| self.total_ticks < self.invalid_until_tick),
            last_move,
        );
        // The shared root grid reserves room for both player bars. A fixed
        // width inside a band bypasses that fit and clips controls at 170%.
        screen = screen.board_with_selection(8, board);
        let mut you_slots: Vec<BuilderSlot> = vec![(
            SlotWidth::Fill,
            Box::new(move |slot| {
                slot.text(you_name).secondary(format!(
                    "{you_color} · {turn} · {game_kind} · {move_status}{check}"
                ))
            }),
        )];
        if timed {
            you_slots.push((
                SlotWidth::Fixed(220),
                Box::new(move |slot| slot.chips([("your-clock", you_clock, your_active)])),
            ));
        } else {
            you_slots.push((
                SlotWidth::Fixed(220),
                Box::new(|slot| slot.chips([("take-back", "Undo", false)])),
            ));
        }
        screen = screen.band(BandAlign::Top, you_slots);
        if !game.active() && !self.result_dismissed {
            let result = game.result();
            // Against the computer the next thing an owner wants is usually
            // another game, which otherwise took Back, Computer and Start.
            if self.local_game {
                return screen
                    .modal(result, |builder| {
                        builder
                            .primary_button("computer-again", "New game")
                            .button("dismiss-result", "See the board")
                    })
                    .build();
            }
            return screen
                .modal("", move |builder| {
                    builder.primary_button("dismiss-result", result)
                })
                .build();
        }
        if let Some(promotion) = &self.promotion {
            let white = chess::piece_at(&game.fen, &promotion.from)
                .is_some_and(|piece| piece.is_ascii_uppercase());
            let cells = promotion
                .choices
                .iter()
                .filter_map(|choice| {
                    let piece = if white {
                        choice.to_ascii_uppercase()
                    } else {
                        *choice
                    };
                    Some((
                        format!("promote-{choice}"),
                        match choice {
                            'q' => "Queen",
                            'r' => "Rook",
                            'b' => "Bishop",
                            'n' => "Knight",
                            _ => return None,
                        },
                        piece_glyph(piece),
                    ))
                })
                .collect::<Vec<_>>();
            let movement = format!("{} → {}", promotion.from, promotion.to);
            screen = screen.modal("Choose promotion", move |builder| {
                builder.secondary(movement).board(4, cells)
            });
        } else if let Some(confirmation) = self.confirmation {
            screen = match confirmation {
                Confirmation::Resign => screen.confirm(
                    "Resign?",
                    "Final",
                    ("resign", "Resign"),
                    ("cancel-confirm", "Cancel"),
                ),
                Confirmation::Abort => screen.confirm(
                    "Abort?",
                    "Opening only",
                    ("abort", "Abort"),
                    ("cancel-confirm", "Cancel"),
                ),
            };
        } else if let Some(action) = self
            .pending_action
            .as_ref()
            .filter(|action| !matches!(action, GameAction::Move(_)))
        {
            // A move gets no dialog. It used to raise one when sent and drop it
            // when confirmed, two full repaints of an e-ink panel for every
            // move, on top of the board change itself. The status line and the
            // outlined squares say the same thing in place.
            screen = screen.modal(action.label(), |builder| builder.secondary("Sending"));
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
        let revalidating = matches!(self.account, AccountState::Ready(_));
        self.cancel_account_retry(context);
        if !revalidating {
            self.account = AccountState::Checking;
            self.playing_ready = false;
            self.notice = None;
        }
        if self
            .spawn(context, Pending::Account, api::account(), true)
            .is_none()
        {
            if revalidating {
                self.schedule_account_retry(context);
            } else {
                self.account = AccountState::Unknown;
                self.notice = Some(
                    "Finishing previous requests. Refresh account again in a moment.".to_owned(),
                );
            }
        }
    }

    fn cancel_account_retry(&mut self, context: &mut Context) {
        for (task, pending) in self.tasks.clone() {
            if matches!(pending, Pending::AccountRetry) {
                context.cancel(task);
                self.tasks.remove(&task);
                self.retired_tasks.insert(task, pending);
            }
        }
    }

    fn schedule_account_retry(&mut self, context: &mut Context) {
        // Two stream reads plus the game clock leave one slot for the owner.
        // Credential polling would consume that slot for fifteen seconds.
        if ((self.session.is_some() || self.seek_waiting)
            && matches!(self.account, AccountState::Ready(_)))
            || self.suspended
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
            .is_some_and(|(expected, _, _)| expected != &id)
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
        self.local_game = false;
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
        self.cancel_account_retry(context);
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
            self.notice = Some("Opening your game. Please wait.".to_owned());
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
        self.result_dismissed = false;
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
            self.explain_unready_seek(context);
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
        self.cancel_account_retry(context);
        self.selected_preset = Some(preset);
        self.selected_rated = self.seek_rated;
        self.seek_candidate = None;
        if let Some(task) = self.spawn(
            context,
            Pending::Seek { generation, preset },
            api::seek(preset, self.selected_rated),
            false,
        ) {
            self.seek_task = Some(task);
            self.route = Route::Pairing;
            self.notice = None;
            self.reset_clock(context, true);
            Self::keep_live(context);
            self.schedule_live_seek_check(context);
        } else {
            self.seek_waiting = false;
            self.selected_preset = None;
            self.seek_baseline.clear();
        }
    }

    /// Answers a time tile tapped while a seek cannot start.
    ///
    /// Each reason gets the next useful step rather than one word: a seek
    /// already running is shown, a missing account leads to where it is
    /// connected, an account that could not be checked is checked again, and
    /// anything else is named along with when to tap again.
    fn explain_unready_seek(&mut self, context: &mut Context) {
        if self.seek_task.is_some() || self.seek_waiting {
            self.route = Route::Pairing;
            return;
        }
        match self.account {
            AccountState::Missing | AccountState::Invalid => {
                // Account/Games already says what is missing and holds the
                // Connect button, so landing there is the whole answer.
                self.route = Route::Play;
                return;
            }
            AccountState::Unknown | AccountState::Failed(_) | AccountState::Checking => {
                self.validate_account(context);
                if self.account == AccountState::Checking {
                    self.notice = Some(
                        "Checking your Lichess account. Tap a time again when the top line says Ready."
                            .to_owned(),
                    );
                }
                return;
            }
            AccountState::Ready(_) => {}
        }
        if self.pending_action.is_some() || self.accepted_challenge.is_some() {
            self.notice = Some("Wait for the current server action to finish.".to_owned());
        } else if self.game.as_ref().is_some_and(Game::active) {
            self.notice = Some(if self.local_game {
                "Finish or resign the computer game first. Tap Computer to return to it.".to_owned()
            } else {
                "Finish your current game first. It is under Account/Games.".to_owned()
            });
        } else {
            self.notice = Some(
                "Still connecting to Lichess. Tap a time again when the top line says Ready."
                    .to_owned(),
            );
        }
    }

    fn schedule_live_seek_check(&mut self, context: &mut Context) {
        if self.suspended
            || !self.seek_waiting
            || self.seek_task.is_none()
            || self.has_pending(|pending| {
                matches!(
                    pending,
                    Pending::SeekGrace { .. } | Pending::SeekReconcile { .. }
                )
            })
        {
            return;
        }
        // Cancellation frees capacity asynchronously. Retry scheduling when a
        // retired task settles, without ever submitting another seek.
        let _ = self.spawn(
            context,
            Pending::SeekGrace {
                generation: self.seek_generation,
            },
            Task::Sleep { seconds: 10 },
            false,
        );
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
            self.notice = Some("Cancelling.".to_owned());
        } else if self.route == Route::Pairing {
            self.clock.stop(context);
            self.route = Route::Play;
            self.notice = Some("Cancelled.".to_owned());
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
        self.expected_seek_game = Some((candidate.id.clone(), preset, self.selected_rated));
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
        self.notice = None;
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
                } | Pending::SeekReconcile {
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

    fn reconcile_seek(&mut self, context: &mut Context, generation: u64) {
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
                !self.seek_baseline.contains(&summary.id)
                    && preset.matches_summary(summary, self.selected_rated)
            });
            let first = candidates.next().cloned();
            let ambiguous = candidates.next().is_some();
            if !ambiguous {
                self.seek_candidate = first;
            }
            if ambiguous {
                if let Some(task) = self.seek_task.take() {
                    context.cancel(task);
                }
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
        if self.seek_candidate.is_some() {
            self.open_seek_candidate(context);
        } else if self.seek_task.is_some() {
            self.notice = None;
            let _ = self.spawn(
                context,
                Pending::SeekGrace { generation },
                Task::Sleep { seconds: 10 },
                false,
            );
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

    fn start_computer_game(&mut self, context: &mut Context) {
        let owner = match &self.account {
            AccountState::Ready(account) => account.username.clone(),
            _ => "You".to_owned(),
        };
        self.notice = None;
        let color = self.computer_side.resolve();
        let clock_ms = self.computer_clock.milliseconds();
        let you = Player {
            id: None,
            name: owner,
            rating: None,
        };
        let kobo = Player {
            id: None,
            name: "Kobo".to_owned(),
            rating: None,
        };
        let (white, black) = match color {
            Color::White => (you, kobo),
            Color::Black => (kobo, you),
        };
        let state = ServerState {
            moves: Vec::new(),
            white_ms: clock_ms.unwrap_or(0),
            black_ms: clock_ms.unwrap_or(0),
            white_increment_ms: 0,
            black_increment_ms: 0,
            status: "started".to_owned(),
            winner: None,
            white_draw: false,
            black_draw: false,
            white_takeback: false,
            black_takeback: false,
        };
        self.game = Game::from_full(
            FullGame {
                id: "computer".to_owned(),
                initial_fen: "startpos".to_owned(),
                rated: false,
                speed: "offline".to_owned(),
                white,
                black,
                state,
            },
            color,
        );
        if let Some(game) = self.game.as_mut() {
            game.timed = clock_ms.is_some();
            // Playing Black means the computer opens, before the board is
            // first drawn, so the owner never sees a board waiting on nobody.
            if !game.my_turn() && !Self::play_computer_reply(game) {
                self.notice = Some("Computer move failed".to_owned());
            }
        }
        self.session = Some(Session {
            game_id: "computer".to_owned(),
            color,
            opponent: "Kobo".to_owned(),
            rated: false,
        });
        self.local_game = true;
        self.board_open = Some("computer".to_owned());
        self.board_ready = true;
        self.selected = None;
        self.result_dismissed = false;
        self.route = Route::Game;
        self.reset_clock(context, self.local_clock_runs());
    }

    /// Takes back the owner's last move against the computer, and the reply.
    ///
    /// Offered only without a clock, where there is no time to put back. A
    /// finished game can be taken back too, since undoing the move that lost
    /// is most of why an offline player reaches for it. The board is rebuilt
    /// from the start rather than unwound, because the game only ever moves
    /// forward and replaying a short move list is cheap.
    fn take_back(&mut self) {
        if !self.local_game {
            return;
        }
        let Some(game) = self.game.as_ref().filter(|game| !game.timed) else {
            return;
        };
        let owner_moves_first = game.my_color == Color::White;
        let keep = (0..game.state.moves.len())
            .rev()
            .find(|plies| (plies % 2 == 0) == owner_moves_first);
        let Some(keep) = keep else {
            self.notice = Some("Nothing to take back yet.".to_owned());
            return;
        };
        let state = ServerState {
            moves: game.state.moves[..keep].to_vec(),
            status: "started".to_owned(),
            winner: None,
            white_draw: false,
            black_draw: false,
            ..game.state.clone()
        };
        let rebuilt = Game::from_full(
            FullGame {
                id: game.id.clone(),
                initial_fen: game.initial_fen.clone(),
                rated: false,
                speed: game.speed.clone(),
                white: game.white.clone(),
                black: game.black.clone(),
                state,
            },
            game.my_color,
        );
        let Some(mut rebuilt) = rebuilt else {
            self.notice = Some("Could not take the move back.".to_owned());
            return;
        };
        rebuilt.timed = false;
        self.game = Some(rebuilt);
        self.selected = None;
        self.invalid_square = None;
        self.promotion = None;
        self.confirmation = None;
        self.menu_open = false;
        self.result_dismissed = false;
        self.notice = None;
    }

    /// Whether the one-second heartbeat should run for the computer game.
    ///
    /// Only a clocked game in progress needs it. An unclocked one has nothing
    /// that changes with time, and every heartbeat repaints the screen.
    fn local_clock_runs(&self) -> bool {
        self.game
            .as_ref()
            .is_some_and(|game| game.active() && game.timed)
    }

    /// Plays the computer's move, charging its thinking time to its clock.
    /// Returns false only when the move it chose could not be applied.
    fn play_computer_reply(game: &mut Game) -> bool {
        let started = Instant::now();
        let Some(reply) = chess::tiny_move(&game.fen) else {
            Self::finish_local_position(game);
            return true;
        };
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if game.commit_turn_time(elapsed_ms) {
            return true;
        }
        let mut state = game.state.clone();
        state.moves.push(reply);
        if game.apply(state).is_none() {
            return false;
        }
        Self::finish_local_position(game);
        true
    }

    fn finish_local_position(game: &mut Game) {
        let Some((status, winner)) = chess::terminal(&game.fen) else {
            return;
        };
        game.state.status = status.to_owned();
        game.state.winner = winner.and_then(|color| match color {
            'w' => Some(Color::White),
            'b' => Some(Color::Black),
            _ => None,
        });
    }

    fn apply_local_move(&mut self, context: &mut Context, movement: String) {
        let elapsed_ms = u64::try_from(self.clock.waited().as_millis()).unwrap_or(u64::MAX);
        let active = {
            let Some(game) = self.game.as_mut().filter(|game| game.active()) else {
                return;
            };
            if game.commit_turn_time(elapsed_ms) {
                false
            } else {
                let mut state = game.state.clone();
                state.moves.push(movement);
                if game.apply(state).is_none() {
                    self.notice = Some("Move failed".to_owned());
                    return;
                }
                Self::finish_local_position(game);
                if game.active() && !Self::play_computer_reply(game) {
                    self.notice = Some("Computer move failed".to_owned());
                    return;
                }
                game.active()
            }
        };
        self.selected = None;
        self.promotion = None;
        self.pending_move = None;
        self.notice = None;
        if !active {
            self.result_dismissed = false;
        }
        self.reset_clock(context, self.local_clock_runs());
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
                    self.notice = Some("Reconnecting to Lichess.".to_owned());
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

    fn flash_invalid(&mut self, square: String) {
        self.invalid_square = Some(square);
        self.invalid_until_tick = self.total_ticks.saturating_add(2);
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
            self.notice = Some("Reconnecting to Lichess.".to_owned());
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
                self.invalid_square = None;
                self.notice = None;
            } else {
                self.flash_invalid(square);
                self.notice = Some("Your piece".to_owned());
            }
            return;
        };
        if from == square {
            self.invalid_square = None;
            self.notice = None;
            return;
        }
        if chess::piece_belongs_to(&game.fen, &square, game.my_color.fen()) {
            self.selected = Some(square);
            self.invalid_square = None;
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
            self.selected = Some(from);
            self.flash_invalid(square);
            self.notice = Some("Illegal".to_owned());
            return;
        }
        self.invalid_square = None;
        self.notice = None;
        if self.local_game {
            self.apply_local_move(context, movement);
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
            if solver.is_some_and(|color| chess::piece_belongs_to(&puzzle.fen, &square, color)) {
                self.selected = Some(square);
                self.invalid_square = None;
                self.notice = None;
            } else {
                self.invalid_square = Some(square);
                self.invalid_until_tick = self.total_ticks.saturating_add(2);
                self.notice = Some("Your piece".to_owned());
            }
            return;
        };
        if from == square {
            self.invalid_square = None;
            return;
        }
        if solver.is_some_and(|color| chess::piece_belongs_to(&puzzle.fen, &square, color)) {
            self.selected = Some(square);
            self.invalid_square = None;
            self.notice = None;
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
            self.selected = Some(from);
            self.invalid_square = Some(square);
            self.invalid_until_tick = self.total_ticks.saturating_add(2);
            self.notice = Some("Illegal".to_owned());
            return;
        };
        self.invalid_square = None;
        if expected
            .as_ref()
            .is_some_and(|solution| solution != &movement)
        {
            self.puzzle_wrong = self.puzzle_wrong.saturating_add(1);
            self.notice = Some(if self.puzzle_wrong == 1 {
                "Not it. Try again.".to_owned()
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

    fn encode_preferences(&self) -> Vec<u8> {
        ObjectBuilder::new()
            .set("version", 1_u32)
            .set("seek_rated", self.seek_rated)
            .set("computer_side", self.computer_side.api_value())
            .set("computer_clock", self.computer_clock.stored())
            .build()
            .to_json()
            .into_bytes()
    }

    /// Reads the saved choices all or nothing, so a file from a later
    /// version or a damaged one leaves every default in place rather than
    /// half of them.
    fn decode_preferences(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > 1024 {
            return false;
        }
        let Some(value) = std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| kobo_json::parse(text).ok())
        else {
            return false;
        };
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return false;
        }
        let rated = value.get("seek_rated").and_then(Value::as_bool);
        let side = value
            .get("computer_side")
            .and_then(Value::as_str)
            .and_then(ChallengeSide::from_api_value);
        let clock = value
            .get("computer_clock")
            .and_then(Value::as_str)
            .and_then(ComputerClock::from_stored);
        let (Some(rated), Some(side), Some(clock)) = (rated, side, clock) else {
            return false;
        };
        self.seek_rated = rated;
        self.computer_side = side;
        self.computer_clock = clock;
        true
    }

    fn save_preferences(&self, context: &mut Context) {
        context
            .store()
            .save(PREFERENCES_KEY, self.encode_preferences());
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
        let Some((expected_id, preset, rated)) = self.expected_seek_game.clone() else {
            return true;
        };
        if expected_id != id {
            return true;
        }
        self.expected_seek_game = None;
        if preset.matches_full(full, rated) {
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
        if self.local_game {
            self.close_network_reads_preserving_local_game(context, true);
            self.schedule_account_retry(context);
            return;
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
                    && self.selected_preset.is_some_and(|preset| {
                        preset.matches_summary(&summary, self.selected_rated)
                    })
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
                    self.seek_candidate = Some(summary);
                    self.open_seek_candidate(context);
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
                    self.notice = Some("This game finished while you were away.".to_owned());
                    self.discard_game(context, &id);
                } else if self.game.as_ref().is_some_and(|game| game.id == id) {
                    self.notice = None;
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
                    self.notice = Some("Could not load the position. Reconnecting.".to_owned());
                    self.close_board(context, id);
                    self.schedule_board_reconnect(context, id);
                    return;
                };
                self.game = Some(game);
                self.result_dismissed = false;
                self.board_ready = true;
                self.board_backoff = 1;
                self.notice = None;
                self.reconcile_pending_move();
                if pending.is_some() && self.pending_move.is_none() {
                    self.notice = Some("Board updated.".to_owned());
                } else if pending.is_some() {
                    self.pending_move = None;
                    self.selected = None;
                    self.notice =
                        Some("The move was not confirmed. Choose your move again.".to_owned());
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
                        self.notice =
                            Some("The board changed. Refreshing the position.".to_owned());
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
                    "The {variant} variant is not supported. Choose a standard game."
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
            self.notice = Some("The position changed before your move was confirmed.".to_owned());
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
        self.schedule_account_retry(context);
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
            self.notice = Some("Your challenge started. Opening the game.".to_owned());
            self.open_board(context, summary.session());
            true
        } else {
            if self.route == Route::Challenge {
                self.route = Route::Play;
            }
            self.notice = Some(if ambiguous {
                "Several games found. Choose from Ongoing games.".to_owned()
            } else {
                "That challenge is no longer active. You can choose another game.".to_owned()
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
                            "Lichess asked us to wait {seconds}s. We will check for your game again."
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
                    self.schedule_account_retry(context);
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
                    if self.seek_waiting && self.seek_candidate.is_none() {
                        if let Some(preset) = self.selected_preset {
                            let mut candidates = games.iter().filter(|summary| {
                                !self.seek_baseline.contains(&summary.id)
                                    && preset.matches_summary(summary, self.selected_rated)
                            });
                            let first = candidates.next().cloned();
                            if candidates.next().is_none() {
                                self.seek_candidate = first;
                            }
                        }
                    }
                    self.summaries = games;
                    if self.seek_candidate.is_some() {
                        self.open_seek_candidate(context);
                    }
                    let recovered = self.reconcile_accepted_challenge_from_playing(context);
                    if !recovered && !self.local_game {
                        if let Some(session) = self.session.clone() {
                            if let Some(summary) = self
                                .summaries
                                .iter()
                                .find(|summary| summary.id == session.game_id)
                                .cloned()
                            {
                                self.open_board(context, summary.session());
                            } else {
                                self.notice = None;
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
                    self.notice = Some("Could not connect to Lichess. Trying again.".to_owned());
                }
            }
            Pending::EventNext => {
                if let Some(event) = api::parse_event(bytes) {
                    self.handle_event(context, event);
                    self.next_event(context);
                } else {
                    self.notice = Some("Could not read the game update. Reconnecting.".to_owned());
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
                self.reconcile_seek(context, generation);
            }
            Pending::SeekReconcile { generation } => {
                if self.seek_waiting && self.seek_generation == generation {
                    if let Some(games) = api::parse_playing(bytes) {
                        self.finish_seek_reconciliation(context, generation, games);
                    } else {
                        self.notice = Some(
                            "Could not check your game. Wait for another check or cancel pairing."
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
                    self.notice = Some("Could not open your game. Trying again.".to_owned());
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
                    self.notice = Some("Could not read the board update. Reconnecting.".to_owned());
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
            Pending::CreateChallenge { username } => {
                self.challenge_keyboard.clear();
                self.challenge_username.clear();
                if self.route == Route::ChallengeSetup {
                    self.route = Route::Play;
                }
                self.notice = Some(format!("Challenge sent to {username}"));
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
                self.notice = self
                    .pending_move
                    .is_none()
                    .then(|| "Waiting for Lichess.".to_owned());
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
                        "That game is no longer available. Choose another from Ongoing games."
                            .to_owned(),
                    );
                    self.discard_game(context, &id);
                } else {
                    self.notice = Some(
                        if matches!(
                            error,
                            TaskError::Unreachable | TaskError::TimedOut | TaskError::Offline
                        ) {
                            "Reconnecting to Lichess.".to_owned()
                        } else {
                            Failure::of(error).naming(api::SECRET)
                        },
                    );
                    self.schedule_board_reconnect(context, &id);
                }
            }
            Pending::Seek { generation, preset }
                if error == TaskError::Denied
                    && !self.selected_rated
                    && self.seek_is_current(generation, preset) =>
            {
                // The runtime signs Lichess requests only for bodies it lists,
                // and a runtime older than casual seeks lists only rated ones.
                // The seek never left the reader, so there is no game to wait
                // for: stop, and say what would let it through.
                self.seek_task = None;
                self.cancel_seek(context);
                self.clock.stop(context);
                self.route = Route::Play;
                self.notice = Some(
                    "This Cobalt cannot seek casual games yet. Update Cobalt in Settings, or choose Rated."
                        .to_owned(),
                );
            }
            Pending::Seek { generation, preset } => {
                if self.route == Route::Pairing && self.seek_is_current(generation, preset) {
                    self.await_seek_event(
                        context,
                        generation,
                        preset,
                        format!(
                            "{} Checking whether your game started.",
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
                        "{} We will check for your game again.",
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
                    "{} Checking whether the action completed.",
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
            Pending::CreateChallenge { .. } => {
                self.notice = Some(Failure::of(error).naming(api::SECRET));
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
                    self.notice = Some("Cancelled.".to_owned());
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

    fn finish_expired_clock(&mut self, context: &mut Context, elapsed_ms: u64) -> bool {
        let expired = self
            .game
            .as_mut()
            .is_some_and(|game| game.expire_clock(elapsed_ms));
        if !expired {
            return false;
        }
        self.clear_pending_action();
        self.pending_move = None;
        self.selected = None;
        self.promotion = None;
        self.notice = None;
        self.result_dismissed = false;
        self.menu_open = false;
        self.clock.stop(context);
        true
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

    fn close_network_reads_preserving_local_game(
        &mut self,
        context: &mut Context,
        restart_clock: bool,
    ) {
        let elapsed_ms = u64::try_from(self.clock.waited().as_millis()).unwrap_or(u64::MAX);
        if self
            .game
            .as_mut()
            .is_some_and(|game| game.commit_turn_time(elapsed_ms))
        {
            self.result_dismissed = false;
        }
        let board = self
            .board_open
            .take()
            .or_else(|| self.game.as_ref().map(|game| game.id.clone()));
        self.close_live_reads(context);
        self.board_open = board;
        self.board_ready = self.game.is_some();
        if restart_clock {
            self.reset_clock(context, self.local_clock_runs());
        }
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
            "result" => {
                if !self.install_demo("game") {
                    return false;
                }
                if let Some(game) = self.game.as_mut() {
                    "mate".clone_into(&mut game.state.status);
                    game.state.winner = Some(Color::White);
                }
            }
            "timeout" => {
                if !self.install_demo("game") {
                    return false;
                }
                if let Some(game) = self.game.as_mut() {
                    "outoftime".clone_into(&mut game.state.status);
                    game.state.winner = Some(Color::White);
                    game.state.black_ms = 0;
                }
            }
            "promotion" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.session = Some(Session {
                    game_id: "demoPromo".to_owned(),
                    color: Color::White,
                    opponent: "KnightReader".to_owned(),
                    rated: false,
                });
                self.game = Game::from_full(
                    FullGame {
                        id: "demoPromo".to_owned(),
                        initial_fen: "7k/P7/8/8/8/8/8/7K w - - 0 1".to_owned(),
                        rated: false,
                        speed: "rapid".to_owned(),
                        white: Player {
                            id: Some("demo-owner".to_owned()),
                            name: "DemoOwner".to_owned(),
                            rating: Some(1510),
                        },
                        black: Player {
                            id: Some("other".to_owned()),
                            name: "KnightReader".to_owned(),
                            rating: Some(1542),
                        },
                        state: ServerState {
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
                        },
                    },
                    Color::White,
                );
                self.board_open = Some("demoPromo".to_owned());
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
            "pairing-casual" => {
                if !self.install_demo("pairing") {
                    return false;
                }
                self.seek_rated = false;
                self.selected_rated = false;
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
            "pairing-error" => {
                if !self.install_demo("reconciling") {
                    return false;
                }
                self.tasks.clear();
                self.tasks
                    .insert(TaskId(998), Pending::SeekGrace { generation: 1 });
                self.notice = Some(
                    "Could not check your game. Wait for another check or cancel pairing."
                        .to_owned(),
                );
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
            "challenge-player" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                self.challenge_keyboard = Keyboard::with_text("KnightReader");
                self.route = Route::ChallengePlayer;
            }
            "challenge-setup" => {
                self.account = AccountState::Ready(Account {
                    id: "demo-owner".to_owned(),
                    username: "DemoOwner".to_owned(),
                });
                "KnightReader".clone_into(&mut self.challenge_username);
                self.challenge_preset = SeekPreset::Rapid10_5;
                self.challenge_side = ChallengeSide::Random;
                self.route = Route::ChallengeSetup;
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
        context.set_orientation(Orientation::Portrait);
        let demo = std::env::var("KOBO_LICHESS_DEMO").ok();
        #[cfg(debug_assertions)]
        let demo = demo.or_else(|| option_env!("KOBO_LICHESS_DEMO_BUILD").map(str::to_owned));
        if demo
            .as_deref()
            .is_some_and(|scenario| self.install_demo(scenario))
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
        context.store().load(PREFERENCES_KEY);
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
                        Some("Could not restore your last game. Check Ongoing games.".to_owned());
                }
            } else if key == PUZZLE_KEY {
                self.loaded_puzzles = true;
                if let Some(bytes) = value {
                    if !self.decode_puzzles(&bytes) {
                        context.store().forget(PUZZLE_KEY);
                        self.notice = Some(
                            "Saved puzzles could not be read. Download them again.".to_owned(),
                        );
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
                            Some("Saved connection settings could not be read. Checking your game again.".to_owned());
                    }
                }
            } else if key == EVENT_RATE_KEY {
                self.loaded_event_rate = true;
                if let Some(value) = value {
                    if let Some(not_before) = decode_rate_deadline(&value) {
                        self.event_rate_limit = Some(not_before);
                    } else {
                        context.store().forget(EVENT_RATE_KEY);
                        self.notice = Some(
                            "Saved connection settings could not be read. Connecting again."
                                .to_owned(),
                        );
                    }
                }
            } else if key == PREFERENCES_KEY {
                // A choice is a convenience: one that cannot be read is
                // dropped quietly, and the defaults stand until the owner
                // picks again.
                if value
                    .as_deref()
                    .is_some_and(|bytes| !self.decode_preferences(bytes))
                {
                    context.store().forget(PREFERENCES_KEY);
                }
            } else if key == SEEK_RATE_KEY {
                self.loaded_seek_rate = true;
                if let Some(value) = value {
                    if let Some(not_before) = decode_rate_deadline(&value) {
                        self.seek_rate_limit = Some(not_before);
                    } else {
                        context.store().forget(SEEK_RATE_KEY);
                        self.notice =
                            Some("Saved pairing settings could not be read. Check your account before pairing.".to_owned());
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
        if self.credential.is_open() {
            if self.credential.on_action(context, action).is_some() {
                self.show(context);
            }
            return;
        }
        if action == ActionId::BACK {
            if self.route == Route::ChallengePlayer {
                self.challenge_keyboard.clear();
            }
            if self.route == Route::Game
                && self.game.as_ref().is_some_and(|game| !game.active())
                && !self.result_dismissed
            {
                self.result_dismissed = true;
            } else if self.promotion.take().is_some() {
                self.selected = None;
            } else if self.confirmation.take().is_some() || self.menu_open {
                self.menu_open = false;
            } else if self.pending_action.is_some()
                || self.pending_move.is_some()
                || self.accepted_challenge.is_some()
                || self.has_pending(|pending| matches!(pending, Pending::CreateChallenge { .. }))
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
                    Route::Game if self.local_game => Route::Home,
                    Route::Game
                    | Route::Pairing
                    | Route::ChallengePlayer
                    | Route::ChallengeSetup
                    | Route::Challenge => Route::Play,
                    _ => Route::Home,
                };
            }
        } else if self.route == Route::ChallengePlayer {
            match self.challenge_keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let username = self.challenge_keyboard.text().trim().to_owned();
                    if api::challenge_player(
                        &username,
                        self.challenge_preset,
                        self.challenge_side.api_value(),
                    )
                    .is_some()
                    {
                        self.challenge_username = username;
                        self.notice = None;
                        self.route = Route::ChallengeSetup;
                    } else {
                        self.notice = Some("Enter a valid Lichess username".to_owned());
                    }
                }
                Some(Pressed::Edited | Pressed::Shifted) => {
                    self.notice = None;
                }
                None => {}
            }
        } else if action == action_id("puzzles") {
            self.home_page = 0;
            self.route = Route::Puzzles;
        } else if action == action_id("how-to-play") {
            self.home_page = 0;
            self.route = Route::HowTo;
        } else if action == action_id("home") {
            self.home_page = 0;
            self.route = Route::Home;
        } else if action == action_id("play") {
            self.home_page = 0;
            self.route = Route::Play;
            self.validate_account(context);
        } else if action == action_id("play-computer") {
            if self.local_game && self.game.as_ref().is_some_and(Game::active) {
                self.route = Route::Game;
                self.reset_clock(context, self.local_clock_runs());
            } else {
                self.notice = None;
                self.route = Route::ComputerSetup;
            }
        } else if action == action_id("start-computer") {
            self.start_computer_game(context);
        } else if let Some(side) = computer_side_action(action) {
            self.computer_side = side;
            self.save_preferences(context);
        } else if let Some(clock) = computer_clock_action(action) {
            self.computer_clock = clock;
            self.save_preferences(context);
        } else if action == action_id("seek-rated") || action == action_id("seek-casual") {
            self.seek_rated = action == action_id("seek-rated");
            self.save_preferences(context);
        } else if action == action_id("home-next") || action == action_id("home-previous") {
            let pages = self.home_pages(context).len();
            self.home_page = if action == action_id("home-next") {
                (self.home_page + 1).min(pages.saturating_sub(1))
            } else {
                self.home_page.saturating_sub(1)
            };
        } else if action == action_id("refresh-play") {
            self.validate_account(context);
        } else if action == action_id(CONNECT) {
            self.credential.open();
        } else if action == action_id("challenge-player") {
            self.challenge_keyboard.clear();
            self.challenge_username.clear();
            self.challenge_preset = SeekPreset::Rapid10_0;
            self.challenge_side = ChallengeSide::Random;
            self.notice = None;
            self.route = Route::ChallengePlayer;
        } else if let Some(preset) = challenge_preset_action(action) {
            self.challenge_preset = preset;
        } else if let Some(side) = challenge_side_action(action) {
            self.challenge_side = side;
        } else if action == action_id("send-player-challenge") {
            let username = self.challenge_username.clone();
            if !self.has_pending(|pending| matches!(pending, Pending::CreateChallenge { .. })) {
                if let Some(work) = api::challenge_player(
                    &username,
                    self.challenge_preset,
                    self.challenge_side.api_value(),
                ) {
                    let _ = self.spawn(context, Pending::CreateChallenge { username }, work, false);
                }
            }
        } else if action == action_id("opponent-clock") || action == action_id("your-clock") {
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
                if self.local_game {
                    self.board_open = Some(session.game_id);
                    self.board_ready = self.game.is_some();
                    self.route = Route::Game;
                    self.reset_clock(context, self.local_clock_runs());
                } else if self
                    .game
                    .as_ref()
                    .is_none_or(|game| !self.board_is_live(&game.id))
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
            if self.local_game {
                if let Some(game) = self.game.as_mut().filter(|game| game.active()) {
                    "resign".clone_into(&mut game.state.status);
                    game.state.winner = Some(game.my_color.opposite());
                }
                self.reset_clock(context, false);
            } else if let Some(game) = &self.game {
                self.send_action(context, GameAction::Resign, api::resign(&game.id));
            }
        } else if action == action_id("abort") {
            self.confirmation = None;
            if self.local_game {
                if let Some(game) = self.game.as_mut().filter(|game| game.active()) {
                    "aborted".clone_into(&mut game.state.status);
                }
                self.reset_clock(context, false);
            } else if let Some(game) = &self.game {
                self.send_action(context, GameAction::Abort, api::abort(&game.id));
            }
        } else if action == action_id("offer-draw") {
            if self.local_game {
                if let Some(game) = self.game.as_mut().filter(|game| game.active()) {
                    "draw".clone_into(&mut game.state.status);
                    game.state.winner = None;
                }
                self.reset_clock(context, false);
            } else if let Some(game) = &self.game {
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
            self.selected = None;
        } else if let Some(choice) = promotion_action(action) {
            if let Some(promotion) = self.promotion.take() {
                if promotion.choices.contains(&choice) {
                    let movement = format!("{}{}{}", promotion.from, promotion.to, choice);
                    if self.local_game {
                        self.apply_local_move(context, movement);
                    } else if let Some(game) = self.game.as_ref() {
                        self.send_action(
                            context,
                            GameAction::Move(movement.clone()),
                            api::move_piece(&game.id, &movement),
                        );
                    }
                }
            }
        } else if action == action_id("take-back") {
            self.take_back();
        } else if action == action_id("computer-again") {
            self.result_dismissed = true;
            self.route = Route::ComputerSetup;
        } else if action == action_id("dismiss-result") {
            self.result_dismissed = true;
        } else if action == action_id("new-puzzles") {
            if !self.has_pending(|pending| matches!(pending, Pending::Puzzle)) {
                let _ = self.spawn(context, Pending::Puzzle, api::puzzle(false), true);
                self.notice = Some("Downloading 32 puzzles.".to_owned());
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
                let elapsed_ms = u64::try_from(self.clock.waited().as_millis()).unwrap_or(u64::MAX);
                self.finish_expired_clock(context, elapsed_ms);
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
                self.schedule_live_seek_check(context);
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
        self.schedule_live_seek_check(context);
        self.show(context);
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        let Some(event) = self.credential.on_device_result(&request, &result) else {
            return;
        };
        if event == CredentialEvent::Saved {
            self.notice = None;
            self.validate_account(context);
        }
        self.show(context);
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.suspended = true;
        if self.local_game {
            self.close_network_reads_preserving_local_game(context, false);
        } else {
            self.close_live_reads(context);
        }
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.suspended = false;
        if self.local_game {
            if let Some(game) = &self.game {
                self.board_open = Some(game.id.clone());
                self.board_ready = true;
            }
            self.reset_clock(context, self.local_clock_runs());
        }
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

/// The 64 squares as board cells, from the owner's side.
///
/// The last move is outlined on both of its squares, as the website tints
/// them, because on paper-like ink there is no animation and an opponent's
/// move otherwise has to be found by comparing the board with memory. It
/// borrows the selection outline rather than a new mark, so it is drawn only
/// while nothing is selected: the two never share the board, and tapping a
/// piece swaps one for the other.
fn board_cells(
    fen: &str,
    orientation: Color,
    selected_square: Option<&str>,
    invalid_square: Option<&str>,
    last_move: Option<&str>,
) -> Vec<(String, String, Option<Glyph>, bool)> {
    let ranks: Vec<u8> = match orientation {
        Color::White => (1..=8).rev().collect(),
        Color::Black => (1..=8).collect(),
    };
    let files: Vec<u8> = match orientation {
        Color::White => (b'a'..=b'h').collect(),
        Color::Black => (b'a'..=b'h').rev().collect(),
    };
    let destinations = selected_square
        .map(|from| chess::destinations(fen, from))
        .unwrap_or_default();
    let last_squares = last_move
        .filter(|_| selected_square.is_none())
        .and_then(move_squares)
        .unwrap_or_default();
    let mut cells = Vec::with_capacity(64);
    for rank in ranks {
        for file in &files {
            let square = format!("{}{}", char::from(*file), rank);
            let piece = chess::piece_at(fen, &square);
            let (label, glyph) = if invalid_square == Some(square.as_str()) {
                ("×".to_owned(), None)
            } else if piece.is_none() && destinations.contains(&square) {
                ("·".to_owned(), None)
            } else {
                (" ".to_owned(), piece.and_then(piece_glyph))
            };
            let marked = selected_square == Some(square.as_str())
                || (piece.is_some() && destinations.contains(&square))
                || last_squares.contains(&square.as_str());
            cells.push((format!("square-{square}"), label, glyph, marked));
        }
    }
    cells
}

fn move_squares(movement: &str) -> Option<[&str; 2]> {
    valid_move(movement).then(|| [&movement[0..2], &movement[2..4]])
}

fn piece_glyph(piece: char) -> Option<Glyph> {
    Some(match piece {
        'K' => Glyph::ChessWhiteKing,
        'Q' => Glyph::ChessWhiteQueen,
        'R' => Glyph::ChessWhiteRook,
        'B' => Glyph::ChessWhiteBishop,
        'N' => Glyph::ChessWhiteKnight,
        'P' => Glyph::ChessWhitePawn,
        'k' => Glyph::ChessBlackKing,
        'q' => Glyph::ChessBlackQueen,
        'r' => Glyph::ChessBlackRook,
        'b' => Glyph::ChessBlackBishop,
        'n' => Glyph::ChessBlackKnight,
        'p' => Glyph::ChessBlackPawn,
        _ => return None,
    })
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

fn challenge_preset_action(action: ActionId) -> Option<SeekPreset> {
    SeekPreset::ALL
        .into_iter()
        .find(|preset| action == action_id(&format!("challenge-time-{}", preset.action())))
}

fn challenge_side_action(action: ActionId) -> Option<ChallengeSide> {
    ChallengeSide::ALL
        .into_iter()
        .find(|side| action == action_id(side.action()))
}

fn computer_side_action(action: ActionId) -> Option<ChallengeSide> {
    ChallengeSide::ALL
        .into_iter()
        .find(|side| action == action_id(side.computer_action()))
}

fn computer_clock_action(action: ActionId) -> Option<ComputerClock> {
    ComputerClock::ALL
        .into_iter()
        .find(|clock| action == action_id(clock.action()))
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
        ChallengeDirection, ChallengeSide, ChallengeTime, Color, ComputerClock, Event, FullGame,
        Game, GameAction, Lichess, Orientation, Pending, Player, Promotion, Puzzle, Route,
        ServerState, Session, ACCOUNT_RETRY_SECONDS, BOARD_RATE_KEY, CONNECT, EVENT_RATE_KEY,
        HOME_TILE_COUNT, SEEK_RATE_KEY,
    };
    use kobo_sdk::{
        action_id, ActionId, AppRunner, Command, Context, DeviceRequest, DeviceResult, KoboApp,
        Screen, StoreRequest, TaskError, TaskOutcome,
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
        let screen = app.game_screen(&Context::default());
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
            None,
            None,
        );
        assert_eq!(cells.first().expect("first").0, "square-h1");
        assert_eq!(cells.last().expect("last").0, "square-a8");
        assert_eq!(cells.first().expect("first").2, Some(Glyph::ChessWhiteRook));
        assert_eq!(cells.last().expect("last").2, Some(Glyph::ChessBlackRook));
    }

    #[test]
    fn lichess_requests_portrait_for_the_square_board() {
        let mut runner = AppRunner::new(Lichess::default());
        let commands = runner.start();
        assert!(commands
            .iter()
            .any(|command| { matches!(command, Command::SetOrientation(Orientation::Portrait)) }));
    }

    #[test]
    fn folio_home_pages_every_preset_without_overflow() {
        let expected = std::iter::once(action_id("puzzles"))
            .chain(std::iter::once(action_id("play-computer")))
            .chain(std::iter::once(action_id("how-to-play")))
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
    fn time_tiles_stay_tappable_before_the_account_is_ready() {
        let mut runner = AppRunner::new(Lichess::default());
        let screen = painted(runner.start()).expect("home");
        assert!(format!("{screen:?}").contains("Offline"));
        let tiles = declared_tiles(&screen.nodes);
        assert!(!tiles.is_empty());
        assert!(tiles.iter().all(|(_, _, state, badge_empty)| {
            *badge_empty && matches!(*state, TileState::Normal | TileState::Busy)
        }));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(!layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::TileGlyph(Glyph::Close)));
        assert!(!layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::TileOutline(ControlState::Disabled)));
        let preset = layout
            .nodes
            .iter()
            .find(|node| {
                matches!(
                    node.kind,
                    LayoutKind::Tile(action, _)
                        if action == action_id(api::SeekPreset::Rapid10_0.action())
                )
            })
            .expect("time tile");
        assert_eq!(
            layout.hit_test(
                preset.rect.x + preset.rect.width / 2,
                preset.rect.y + preset.rect.height / 2
            ),
            Some(action_id(api::SeekPreset::Rapid10_0.action()))
        );
    }

    #[test]
    fn a_time_tile_that_cannot_seek_yet_says_why_or_goes_where_the_fix_is() {
        let seek = action_id(api::SeekPreset::Rapid10_0.action());
        let posted_seek = |context: &Context| {
            context.commands().iter().any(|command| {
                matches!(
                    command,
                    Command::Spawn {
                        work: kobo_sdk::Task::Post { url, .. },
                        ..
                    } if url.ends_with("/board/seek")
                )
            })
        };

        let mut missing = Lichess {
            account: AccountState::Missing,
            ..Lichess::default()
        };
        let mut context = Context::default();
        missing.on_action(&mut context, seek);
        assert_eq!(missing.route, Route::Play);
        let account_games = format!("{:?}", missing.play_screen());
        assert!(account_games.contains("Connect a Lichess account to play people"));
        assert!(account_games.contains("Connect Lichess"));
        assert!(!account_games.contains("Challenge player"));
        assert!(!posted_seek(&context));

        let mut unchecked = Lichess::default();
        let mut context = Context::default();
        unchecked.on_action(&mut context, seek);
        assert_eq!(unchecked.account, AccountState::Checking);
        assert!(unchecked.has_pending(|pending| matches!(pending, Pending::Account)));
        assert!(unchecked
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("Tap a time again")));
        assert!(!posted_seek(&context));

        let mut connecting = Lichess {
            event_open: false,
            ..ready_app()
        };
        assert_eq!(connecting.seek_tile_state(), TileState::Busy);
        let mut context = Context::default();
        connecting.on_action(&mut context, seek);
        assert_eq!(connecting.route, Route::Home);
        assert!(connecting
            .notice
            .as_deref()
            .is_some_and(|notice| notice.starts_with("Still connecting")));
        assert!(!posted_seek(&context));

        let mut computer = ready_app();
        let mut context = Context::default();
        computer.start_computer_game(&mut context);
        computer.on_action(&mut context, ActionId::BACK);
        computer.on_action(&mut context, seek);
        assert_eq!(computer.route, Route::Home);
        assert!(computer
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("computer game")));
        assert!(!posted_seek(&context));
    }

    #[test]
    fn casual_is_chosen_in_account_games_and_seeks_from_the_casual_pool() {
        let mut runner = AppRunner::new(ready_app());
        runner.start();
        runner.action(action_id("play"));
        let play = format!("{:?}", runner.app().play_screen());
        assert!(play.contains("Casual") && play.contains("Rated"));
        let commands = runner.action(action_id("seek-casual"));
        assert!(!runner.app().seek_rated);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Save { key, .. }) if key == super::PREFERENCES_KEY
        )));
        let app = runner.app_mut();
        app.event_open = true;
        app.playing_ready = true;
        app.route = Route::Home;
        let commands = runner.action(action_id(api::SeekPreset::Rapid15_10.action()));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Post { url, body, .. },
                ..
            } if url.ends_with("/board/seek")
                && body == "rated=false&time=15&increment=10&variant=standard&color=random"
        )));
        assert!(format!("{:?}", runner.app().pairing_screen()).contains("Rapid · Casual"));

        let mut casual = summary("abcdEF12", api::SeekPreset::Rapid15_10);
        casual.rated = false;
        let rated = summary("ghijKL34", api::SeekPreset::Rapid15_10);
        let app = runner.app_mut();
        let generation = app.seek_generation;
        let mut context = Context::default();
        app.finish_seek_reconciliation(&mut context, generation, vec![rated, casual]);
        assert_eq!(
            app.expected_seek_game,
            Some(("abcdEF12".to_owned(), api::SeekPreset::Rapid15_10, false))
        );
    }

    #[test]
    fn a_casual_seek_the_runtime_refuses_stops_and_says_how_to_get_one() {
        let mut app = Lichess {
            seek_rated: false,
            ..ready_app()
        };
        let mut context = Context::default();
        app.on_action(&mut context, action_id(api::SeekPreset::Rapid10_0.action()));
        assert_eq!(app.route, Route::Pairing);
        let generation = app.seek_generation;
        app.handle_failed(
            &mut context,
            Pending::Seek {
                generation,
                preset: api::SeekPreset::Rapid10_0,
            },
            TaskError::Denied,
        );
        assert_eq!(app.route, Route::Play);
        assert!(!app.seek_waiting && app.seek_task.is_none());
        assert!(app
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("cannot seek casual games")));
        assert!(!app.has_pending(|pending| matches!(pending, Pending::SeekGrace { .. })));
    }

    #[test]
    fn saved_choices_survive_a_restart_and_a_damaged_file_keeps_the_defaults() {
        let mut app = Lichess {
            seek_rated: false,
            computer_side: ChallengeSide::Black,
            computer_clock: ComputerClock::Thirty,
            ..Lichess::default()
        };
        let saved = app.encode_preferences();
        let mut restarted = Lichess::default();
        assert!(restarted.decode_preferences(&saved));
        assert!(!restarted.seek_rated);
        assert_eq!(restarted.computer_side, ChallengeSide::Black);
        assert_eq!(restarted.computer_clock, ComputerClock::Thirty);

        assert!(!app.decode_preferences(
            br#"{"version":1,"seek_rated":true,"computer_side":"purple","computer_clock":"none"}"#
        ));
        assert!(!app.seek_rated);
        assert_eq!(app.computer_side, ChallengeSide::Black);
        assert!(!app.decode_preferences(b"{broken"));
    }

    #[test]
    fn home_copy_is_only_destination_or_time_speed_and_pool() {
        let context = Context::default();
        let mut app = ready_app();
        for (rated, pool) in [(true, "Rated"), (false, "Casual")] {
            app.seek_rated = rated;
            assert_home_copy(&mut app, &context, pool);
        }
    }

    fn assert_home_copy(app: &mut Lichess, context: &Context, pool: &str) {
        for page in 0..app.home_pages(context).len() {
            app.home_page = page;
            let screen = app.home(context);
            let rendered = format!("{screen:?}");
            for forbidden in ["random color", "protocol", "API"] {
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
                } else if action == action_id("play-computer") {
                    assert_eq!(label, "Computer");
                    assert_eq!(subtitle, "Offline");
                } else if action == action_id("how-to-play") {
                    assert_eq!(label, "How to play");
                    assert_eq!(subtitle, "Board · controls");
                } else {
                    let preset = api::SeekPreset::ALL
                        .into_iter()
                        .find(|preset| action == action_id(preset.action()))
                        .expect("preset action");
                    assert_eq!(label, preset.label());
                    assert_eq!(subtitle, format!("{} · {pool}", preset.speed_label()));
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
            let widths = outlines
                .iter()
                .map(|outline| outline.rect.width)
                .collect::<BTreeSet<_>>();
            assert!(
                widths.iter().next_back().expect("widest card")
                    - widths.iter().next().expect("narrowest card")
                    <= 2,
                "Clara {name} card widths were {widths:?}"
            );
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
    fn account_games_opens_a_casual_player_challenge_flow() {
        let mut app = ready_app();
        app.route = Route::Play;
        let play = app.play_screen();
        assert!(format!("{play:?}").contains("Challenge player"));

        let mut context = Context::default();
        app.on_action(&mut context, action_id("challenge-player"));
        assert_eq!(app.route, Route::ChallengePlayer);
        app.challenge_keyboard = kobo_sdk::keyboard::Keyboard::with_text("Reader-Two");
        app.on_action(&mut context, action_id("kb.enter"));
        assert_eq!(app.route, Route::ChallengeSetup);
        assert_eq!(app.challenge_username, "Reader-Two");

        app.on_action(&mut context, action_id("challenge-time-seek-10-5"));
        app.on_action(&mut context, action_id("challenge-side-white"));
        app.on_action(&mut context, action_id("send-player-challenge"));
        let task = context.commands().iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: kobo_sdk::Task::Post { url, body, .. },
            } if url == "https://lichess.org/api/challenge/Reader-Two" => {
                assert_eq!(
                    body,
                    "rated=false&clock.limit=600&clock.increment=5&variant=standard&color=white"
                );
                Some(*task)
            }
            _ => None,
        });
        assert!(task.is_some());
        assert!(app.has_pending(|pending| matches!(
            pending,
            Pending::CreateChallenge { username } if username == "Reader-Two"
        )));
    }

    #[test]
    fn computer_game_plays_locally_without_network_tasks() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("play-computer"));
        assert_eq!(app.route, Route::ComputerSetup);
        app.on_action(&mut context, action_id("start-computer"));
        assert_eq!(app.route, Route::Game);
        assert!(app.local_game);
        assert_eq!(app.game.as_ref().expect("game").opponent().name, "Kobo");
        assert!(!context.commands().iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. } | kobo_sdk::Task::Post { .. },
                ..
            }
        )));

        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e4"));
        let game = app.game.as_ref().expect("game");
        assert_eq!(game.state.moves.first().map(String::as_str), Some("e2e4"));
        assert_eq!(game.state.moves.len(), 2);
        assert!(game.my_turn());
        assert!(!context.commands().iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. } | kobo_sdk::Task::Post { .. },
                ..
            }
        )));
    }

    #[test]
    fn chess_board_is_joined_and_keeps_controls_visible_at_every_text_size() {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut runner = AppRunner::with_metrics(ready_app(), metrics);
            runner.action(action_id("play-computer"));
            let screen = painted(runner.action(action_id("start-computer"))).expect("board screen");
            let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
            assert!(
                !diagnostics.has_errors(),
                "{text_scale:?}: {:?}",
                diagnostics.issues
            );
            let cells: Vec<_> = diagnostics
                .layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
                .collect();
            assert_eq!(cells.len(), 64);
            assert_eq!(
                cells
                    .iter()
                    .filter(|node| matches!(
                        node.kind,
                        LayoutKind::Cell(_, kobo_ui::CellStyle::BoardDark, _)
                    ))
                    .count(),
                32
            );
            assert_eq!(cells[0].rect.x + cells[0].rect.width, cells[1].rect.x);
            assert_eq!(cells[0].rect.y + cells[0].rect.height, cells[8].rect.y);
            let moves = board_cells(super::chess::START, Color::White, Some("e2"), None, None);
            assert_eq!(moves.iter().filter(|cell| cell.1 == "·").count(), 2);
        }
    }

    #[test]
    fn leaving_and_reopening_a_computer_game_preserves_the_position() {
        let mut runner = AppRunner::new(ready_app());
        runner.action(action_id("play-computer"));
        runner.action(action_id("start-computer"));
        runner.action(action_id("square-e2"));
        runner.action(action_id("square-e4"));
        let moves = runner
            .app()
            .game
            .as_ref()
            .expect("computer game")
            .state
            .moves
            .clone();
        assert_eq!(moves.len(), 2);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().route, Route::Home);
        let commands = runner.action(action_id("play-computer"));
        assert_eq!(runner.app().route, Route::Game);
        assert_eq!(
            runner.app().game.as_ref().expect("same game").state.moves,
            moves
        );
        assert!(!commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. } | kobo_sdk::Task::Post { .. },
                ..
            }
        )));
    }

    #[test]
    fn the_puzzles_screen_offers_resume_only_when_a_puzzle_is_ready() {
        let empty = format!("{:?}", Lichess::default().puzzles_screen());
        assert!(empty.contains("Download 32 puzzles"));
        assert!(!empty.contains("Resume puzzle"));

        let fen = super::chess::START.to_owned();
        let ready = Lichess {
            puzzles: vec![Puzzle {
                id: "abc12".to_owned(),
                fen,
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
            }],
            ..Lichess::default()
        };
        assert!(format!("{:?}", ready.puzzles_screen()).contains("Resume puzzle"));
    }

    #[test]
    fn the_computer_opens_as_white_when_the_owner_chooses_black() {
        let mut runner = AppRunner::new(ready_app());
        runner.start();
        runner.action(action_id("play-computer"));
        let setup = format!("{:?}", runner.app().computer_setup_screen());
        for choice in ["White", "Black", "Random", "No clock", "10 min", "30 min"] {
            assert!(setup.contains(choice), "{choice} was not offered");
        }
        runner.action(action_id("computer-side-black"));
        runner.action(action_id("start-computer"));
        let app = runner.app();
        let game = app.game.as_ref().expect("computer game");
        assert_eq!(game.my_color, Color::Black);
        assert_eq!(game.white.name, "Kobo");
        assert_eq!(game.state.moves.len(), 1, "the computer did not open");
        assert!(game.my_turn());
        assert_eq!(app.session.as_ref().expect("session").color, Color::Black);
        let cells = board_cells(&game.fen, Color::Black, None, None, None);
        assert_eq!(cells.first().expect("first").0, "square-h1");
    }

    #[test]
    fn a_computer_game_without_a_clock_shows_none_and_never_ticks() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("play-computer"));
        app.on_action(&mut context, action_id("start-computer"));
        let game = app.game.as_ref().expect("computer game");
        assert!(!game.timed);
        assert!(!app.clock.is_running());
        let chips = chip_actions(&app.game_screen(&Context::default()).nodes);
        assert!(!chips.contains(&action_id("your-clock")));
        assert!(!chips.contains(&action_id("opponent-clock")));
        assert!(chips.contains(&action_id("take-back")));

        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e4"));
        assert_eq!(app.game.as_ref().expect("game").state.moves.len(), 2);
        assert!(!app.clock.is_running());

        app.on_action(&mut context, action_id("confirm-resign"));
        app.on_action(&mut context, action_id("resign"));
        let finished = format!("{:?}", app.game_screen(&Context::default()));
        assert!(finished.contains("New game") && finished.contains("See the board"));
        app.on_action(&mut context, action_id("computer-again"));
        assert_eq!(app.route, Route::ComputerSetup);

        let mut timed = ready_app();
        timed.on_action(&mut context, action_id("play-computer"));
        timed.on_action(&mut context, action_id("computer-clock-10"));
        timed.on_action(&mut context, action_id("start-computer"));
        assert!(timed.game.as_ref().expect("timed game").timed);
        assert!(timed.clock.is_running());
        let screen = timed.game_screen(&Context::default());
        assert!(format!("{screen:?}").contains("10:00"));
        let chips = chip_actions(&screen.nodes);
        assert!(chips.contains(&action_id("your-clock")));
        assert!(!chips.contains(&action_id("take-back")));
    }

    fn chip_actions(nodes: &[Node]) -> BTreeSet<ActionId> {
        let mut actions = BTreeSet::new();
        for node in nodes {
            match node {
                Node::Chips { chips, .. } => actions.extend(chips.iter().map(|chip| chip.action)),
                Node::Band { slots, .. } => {
                    for slot in slots {
                        actions.extend(chip_actions(&slot.nodes));
                    }
                }
                _ => {}
            }
        }
        actions
    }

    #[test]
    fn undo_takes_back_the_owners_move_and_the_reply_and_says_when_there_is_none() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("play-computer"));
        app.on_action(&mut context, action_id("start-computer"));
        app.on_action(&mut context, action_id("take-back"));
        assert_eq!(app.notice.as_deref(), Some("Nothing to take back yet."));

        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e4"));
        app.on_action(&mut context, action_id("confirm-resign"));
        app.on_action(&mut context, action_id("resign"));
        app.on_action(&mut context, action_id("take-back"));
        let game = app.game.as_ref().expect("game");
        assert!(game.state.moves.is_empty());
        assert!(game.active() && game.my_turn() && !game.timed);
        assert_eq!(game.fen, super::chess::START);

        let mut black = ready_app();
        black.computer_side = ChallengeSide::Black;
        black.on_action(&mut context, action_id("play-computer"));
        black.on_action(&mut context, action_id("start-computer"));
        black.on_action(&mut context, action_id("take-back"));
        assert_eq!(black.notice.as_deref(), Some("Nothing to take back yet."));
        assert_eq!(black.game.as_ref().expect("game").state.moves.len(), 1);
        black.on_action(&mut context, action_id("square-e7"));
        black.on_action(&mut context, action_id("square-e5"));
        assert_eq!(black.game.as_ref().expect("game").state.moves.len(), 3);
        black.on_action(&mut context, action_id("take-back"));
        assert_eq!(black.game.as_ref().expect("game").state.moves.len(), 1);
        assert!(black.game.as_ref().expect("game").my_turn());
    }

    #[test]
    fn the_last_move_is_outlined_until_the_owner_selects_a_piece() {
        let fen = super::chess::play(super::chess::START, "e2e4")
            .expect("opening")
            .0;
        let marked = |cells: &[(String, String, Option<Glyph>, bool)]| {
            cells
                .iter()
                .filter(|cell| cell.3)
                .map(|cell| cell.0.clone())
                .collect::<BTreeSet<_>>()
        };
        let cells = board_cells(&fen, Color::Black, None, None, Some("e2e4"));
        assert_eq!(
            marked(&cells),
            ["square-e2", "square-e4"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        let selecting = board_cells(&fen, Color::Black, Some("g8"), None, Some("e2e4"));
        assert_eq!(
            marked(&selecting),
            std::iter::once("square-g8".to_owned()).collect()
        );

        let app = app_with_game(&["e2e4", "e7e5"], Color::White);
        let screen = app.game_screen(&Context::default());
        let outlined = screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Grid { cells, .. } => Some(cells),
                _ => None,
            })
            .flatten()
            .filter(|cell| cell.selected)
            .map(|cell| cell.action)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            outlined,
            [action_id("square-e7"), action_id("square-e5")]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn a_sent_move_waits_in_the_status_line_without_a_dialog() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e4"));
        assert!(app.pending_move.is_some());
        let screen = app.game_screen(&Context::default());
        let rendered = format!("{screen:?}");
        assert!(screen.overlay.is_none(), "a move raised a dialog");
        assert!(!rendered.contains("Submitting move"));
        assert!(rendered.contains("Move sent · Rated · e2e4"));
        let cells = board_cells(
            &app.game.as_ref().expect("game").fen,
            Color::White,
            None,
            None,
            Some("e2e4"),
        );
        assert!(cells
            .iter()
            .filter(|cell| cell.0 == "square-e2" || cell.0 == "square-e4")
            .all(|cell| cell.3));

        app.pending_action = Some(GameAction::Resign);
        let resigning = app.game_screen(&Context::default());
        assert!(resigning.overlay.is_some());
        assert!(format!("{resigning:?}").contains("Resigning"));
    }

    #[test]
    fn computer_game_uses_you_without_a_lichess_account() {
        let mut app = Lichess {
            account: AccountState::Missing,
            ..Lichess::default()
        };
        let mut context = Context::default();
        app.start_computer_game(&mut context);
        let game = app.game.as_ref().expect("game");
        assert_eq!(game.white.name, "You");
        assert_eq!(game.opponent().name, "Kobo");
    }

    #[test]
    fn opening_a_live_board_leaves_offline_mode() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_computer_game(&mut context);
        assert!(app.local_game);

        app.open_board(
            &mut context,
            Session {
                game_id: "abcdEF12".to_owned(),
                color: Color::White,
                opponent: "Other".to_owned(),
                rated: true,
            },
        );

        assert!(!app.local_game);
    }

    #[test]
    fn offline_promotion_is_applied_without_a_network_task() {
        let mut app = Lichess::default();
        assert!(app.install_demo("promotion"));
        app.local_game = true;
        app.promotion = Some(Promotion {
            from: "a7".to_owned(),
            to: "a8".to_owned(),
            choices: vec!['q', 'r', 'b', 'n'],
        });
        let mut context = Context::default();

        app.on_action(&mut context, action_id("promote-q"));

        assert_eq!(
            app.game
                .as_ref()
                .expect("game")
                .state
                .moves
                .first()
                .map(String::as_str),
            Some("a7a8q")
        );
        assert!(!context.commands().iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Post { .. },
                ..
            }
        )));
    }

    #[test]
    fn offline_game_remains_live_across_suspend_and_resume() {
        let mut app = ready_app();
        app.computer_clock = ComputerClock::Ten;
        let mut context = Context::default();
        app.start_computer_game(&mut context);

        app.on_suspend(&mut context);
        assert_eq!(app.board_open.as_deref(), Some("computer"));
        assert!(app.board_ready);
        assert!(!app.clock.is_running());

        app.on_resume(&mut context);
        assert_eq!(app.board_open.as_deref(), Some("computer"));
        assert!(app.board_ready);
        assert!(app.clock.is_running());
    }

    #[test]
    fn account_failure_does_not_interrupt_an_offline_game() {
        let mut app = ready_app();
        app.computer_clock = ComputerClock::Ten;
        let mut context = Context::default();
        app.start_computer_game(&mut context);

        app.account_failure(&mut context, TaskError::NoCredential);

        assert!(matches!(app.account, AccountState::Missing));
        assert_eq!(app.route, Route::Game);
        assert!(app.local_game);
        assert_eq!(app.board_open.as_deref(), Some("computer"));
        assert!(app.board_ready);
        assert!(app.clock.is_running());
    }

    #[test]
    fn saved_online_session_reopens_authoritative_board_in_a_fresh_app() {
        let original = app_with_game(&["e2e4", "e7e5"], Color::White);
        let mut saved = Context::default();
        original.persist_session(&mut saved);
        let bytes = saved
            .commands()
            .iter()
            .find_map(|command| match command {
                Command::Store(StoreRequest::Save { key, value }) if key == super::SESSION_KEY => {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("session persisted through SDK store");
        let mut fresh = ready_app();
        let mut context = Context::default();
        fresh.on_store(
            &mut context,
            kobo_sdk::StoreResult::Loaded {
                key: super::SESSION_KEY.into(),
                value: Some(bytes),
            },
        );
        assert!(
            fresh.game.is_none(),
            "a stored session must not invent board state"
        );
        fresh.on_action(&mut context, action_id("resume-current"));
        assert!(
            fresh
                .tasks
                .values()
                .any(|pending| matches!(pending, Pending::BoardOpen(id) if id == "abcdEF12")),
            "resume must open a stream even without an in-memory board"
        );
        let full = api::parse_board(br#"{"type":"gameFull","id":"abcdEF12","rated":true,"speed":"rapid","variant":{"key":"standard"},"initialFen":"startpos","white":{"id":"owner123","name":"Owner"},"black":{"id":"other123","name":"Other"},"state":{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"started"}}"#, "abcdEF12").unwrap();
        fresh.handle_completed(&mut context, Pending::BoardOpen("abcdEF12".into()), &[]);
        fresh.handle_board(&mut context, "abcdEF12", full);
        assert_eq!(fresh.game.as_ref().unwrap().state.moves, ["e2e4", "e7e5"]);
        assert_eq!(
            fresh.game.as_ref().unwrap().fen,
            original.game.as_ref().unwrap().fen
        );
        assert!(fresh.pending_move.is_none());
        assert!(fresh.pending_action.is_none());
        let finished = api::parse_board(br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"draw"}"#, "abcdEF12").unwrap();
        fresh.handle_board(&mut context, "abcdEF12", finished);
        assert!(fresh.session.is_none());
        assert!(context.commands().iter().any(|command| matches!(command,
            Command::Store(StoreRequest::Forget { key }) if key == super::SESSION_KEY)));
    }

    #[test]
    fn resume_current_restores_an_offline_board_without_network() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_computer_game(&mut context);
        app.board_open = None;
        app.board_ready = false;
        let _ = context.take_commands();

        app.on_action(&mut context, action_id("resume-current"));

        assert!(app.local_game);
        assert_eq!(app.route, Route::Game);
        assert_eq!(app.board_open.as_deref(), Some("computer"));
        assert!(app.board_ready);
        assert!(!context.commands().iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. } | kobo_sdk::Task::Post { .. },
                ..
            }
        )));
    }

    #[test]
    fn completed_player_challenge_does_not_replace_a_newer_screen() {
        let mut app = ready_app();
        app.route = Route::Puzzles;
        let mut context = Context::default();

        app.handle_completed(
            &mut context,
            Pending::CreateChallenge {
                username: "ReaderTwo".to_owned(),
            },
            b"",
        );

        assert_eq!(app.route, Route::Puzzles);
        assert_eq!(app.notice.as_deref(), Some("Challenge sent to ReaderTwo"));
    }

    #[test]
    fn closing_promotion_discards_the_pending_move() {
        let mut app = app_with_game(&[], Color::White);
        app.selected = Some("a7".to_owned());
        app.promotion = Some(Promotion {
            from: "a7".to_owned(),
            to: "a8".to_owned(),
            choices: vec!['q', 'r', 'b', 'n'],
        });
        let mut context = Context::default();
        app.on_action(&mut context, ActionId::BACK);
        assert!(app.promotion.is_none());
        assert!(app.selected.is_none());
        assert!(app.game.as_ref().expect("game").state.moves.is_empty());
    }

    #[test]
    fn computer_game_cancel_keeps_playing_and_draw_finishes_locally() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("play-computer"));
        app.on_action(&mut context, action_id("start-computer"));
        app.on_action(&mut context, action_id("confirm-resign"));
        app.on_action(&mut context, action_id("cancel-confirm"));
        assert!(app.game.as_ref().expect("game").active());
        app.on_action(&mut context, action_id("offer-draw"));
        assert_eq!(app.game.as_ref().expect("game").result(), "Draw agreed");
        assert!(!context.commands().iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. } | kobo_sdk::Task::Post { .. },
                ..
            }
        )));
    }

    #[test]
    fn closing_a_result_keeps_the_final_board_open() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_computer_game(&mut context);
        app.on_action(&mut context, action_id("offer-draw"));
        assert!(app.game_screen(&Context::default()).overlay.is_some());
        app.on_action(&mut context, action_id("dismiss-result"));
        assert_eq!(app.route, Route::Game);
        assert!(app.game.is_some());
        assert!(app.game_screen(&Context::default()).overlay.is_none());
    }

    #[test]
    fn invalid_challenge_username_stays_in_the_keyboard() {
        let mut app = ready_app();
        app.route = Route::ChallengePlayer;
        app.challenge_keyboard = kobo_sdk::keyboard::Keyboard::with_text("../other");
        let mut context = Context::default();
        app.on_action(&mut context, action_id("kb.enter"));
        assert_eq!(app.route, Route::ChallengePlayer);
        assert_eq!(
            app.notice.as_deref(),
            Some("Enter a valid Lichess username")
        );
    }

    #[test]
    fn the_active_game_clock_is_the_dark_chip() {
        fn selected(nodes: &[Node], action: ActionId) -> bool {
            nodes.iter().any(|node| match node {
                Node::Chips { chips, .. } => chips
                    .iter()
                    .any(|chip| chip.action == action && chip.selected),
                Node::Band { slots, .. } => slots.iter().any(|slot| selected(&slot.nodes, action)),
                _ => false,
            })
        }

        let app = app_with_game(&["e2e4"], Color::Black);
        let screen = app.game_screen(&Context::default());
        assert!(!selected(&screen.nodes, action_id("opponent-clock")));
        assert!(selected(&screen.nodes, action_id("your-clock")));
        let other_side = app_with_game(&["e2e4"], Color::White).game_screen(&Context::default());
        assert!(selected(&other_side.nodes, action_id("opponent-clock")));
        assert!(!selected(&other_side.nodes, action_id("your-clock")));
    }

    #[test]
    fn disconnected_and_finished_boards_do_not_claim_the_game_is_paused() {
        let mut app = app_with_game(&["e2e4", "e7e5"], Color::White);
        app.board_open = None;
        app.board_ready = false;
        let disconnected = format!("{:?}", app.game_screen(&Context::default()));
        assert!(disconnected.contains("Reconnecting"));
        assert!(!disconnected.contains("Paused"));
        assert!(disconnected.contains("--:--"));
        app.game.as_mut().unwrap().state.status = "draw".to_owned();
        app.result_dismissed = true;
        let finished = format!("{:?}", app.game_screen(&Context::default()));
        assert!(finished.contains("Finished"));
        assert!(!finished.contains("Reconnect"));
        assert!(!finished.contains("--:--"));
    }

    #[test]
    fn both_game_clocks_fit_the_physical_app_viewport() {
        let metrics = DisplayMetrics {
            height: CLARA_BW_METRICS.height - 58,
            ..CLARA_BW_METRICS
        };
        let app = app_with_game(&["e2e4"], Color::Black);
        let layout = app
            .game_screen(&Context::default())
            .layout_with(&metrics, &Chrome::with_back(true));
        for action in [action_id("opponent-clock"), action_id("your-clock")] {
            let clock = layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::Chip(id, _) if id == action))
                .expect("clock");
            assert!(
                clock.rect.y + clock.rect.height <= metrics.height,
                "clock was clipped below the physical app viewport"
            );
        }
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
                Some(preset.body(true).as_str()),
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
    fn tapping_an_owned_piece_switches_the_selection() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-d2"));
        assert_eq!(app.selected.as_deref(), Some("d2"));
        assert!(app.notice.is_none());
        app.on_action(&mut context, action_id("square-d2"));
        assert!(app.selected.is_none());
    }

    #[test]
    fn illegal_destination_flashes_and_keeps_the_piece_selected() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.on_action(&mut context, action_id("square-e2"));
        app.on_action(&mut context, action_id("square-e5"));

        assert_eq!(app.selected.as_deref(), Some("e2"));
        assert_eq!(app.invalid_square.as_deref(), Some("e5"));
        assert_eq!(app.notice.as_deref(), Some("Illegal"));
        assert!(!context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));

        let cells = board_cells(
            &app.game.as_ref().expect("game").fen,
            Color::White,
            app.selected.as_deref(),
            app.invalid_square.as_deref(),
            None,
        );
        assert_eq!(
            cells
                .iter()
                .find(|(action, _, _, _)| action == "square-e5")
                .map(|(_, label, _, _)| label.as_str()),
            Some("×")
        );
        assert!(cells
            .iter()
            .find(|(action, _, _, _)| action == "square-e2")
            .is_some_and(|(_, _, _, selected)| *selected));

        app.total_ticks = app.invalid_until_tick;
        let cells = board_cells(
            &app.game.as_ref().expect("game").fen,
            Color::White,
            app.selected.as_deref(),
            app.invalid_square
                .as_deref()
                .filter(|_| app.total_ticks < app.invalid_until_tick),
            None,
        );
        assert_eq!(
            cells
                .iter()
                .find(|(action, _, _, _)| action == "square-e5")
                .map(|(_, label, _, _)| label.as_str()),
            Some(" ")
        );
    }

    #[test]
    fn first_tap_must_select_the_players_piece() {
        let mut app = app_with_game(&[], Color::White);
        let mut context = Context::default();
        app.on_action(&mut context, action_id("square-e7"));

        assert!(app.selected.is_none());
        assert_eq!(app.invalid_square.as_deref(), Some("e7"));
        assert_eq!(app.notice.as_deref(), Some("Your piece"));
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
        assert_eq!(app.route, Route::Game);
        assert!(app.seek_candidate.is_none());
        assert_eq!(
            app.expected_seek_game,
            Some(("rightClk".to_owned(), api::SeekPreset::Rapid10_5, true))
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
            .contains("Several games found"));
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
            br#"{"type":"gameStart","game":{"gameId":"abcdEF12","color":"white","rated":true,"speed":"rapid","source":"pool","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"Other"}}}"#,
        )
        .expect("matched game");
        app.handle_event(&mut context, started);
        assert!(!app.seek_waiting);
        assert_eq!(app.route, Route::Game);
        assert!(app.seek_candidate.is_none());
        assert_eq!(
            app.session.as_ref().map(|session| session.game_id.as_str()),
            Some("abcdEF12")
        );
    }

    #[test]
    fn live_seek_checks_for_a_missed_start_without_replaying_the_seek() {
        let mut runner = AppRunner::new(ready_app());
        let mut commands = runner.action(action_id(api::SeekPreset::Rapid10_5.action()));
        let seek = runner.app().seek_task.expect("live seek");
        let grace = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekGrace { generation: 1 }).then_some(*task)
            })
            .expect("check scheduled before seek ends");
        commands.extend(runner.task_outcome(grace, TaskOutcome::Completed(Vec::new())));
        let reconcile = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekReconcile { generation: 1 }).then_some(*task)
            })
            .expect("current games request");
        commands.extend(runner.task_outcome(reconcile, TaskOutcome::Completed(
            br#"{"nowPlaying":[{"gameId":"newGame2","color":"black","rated":true,"source":"lobby","speed":"rapid","variant":{"key":"standard"},"secondsLeft":600,"opponent":{"username":"NewOpponent"}}]}"#.to_vec()
        )));
        assert_eq!(runner.app().route, Route::Game);
        assert!(!runner.app().seek_waiting);
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Cancel(task) if *task == seek)));
        assert!(!runner.app().has_pending(|pending| matches!(
            pending,
            Pending::SeekGrace { .. } | Pending::SeekReconcile { .. }
        )));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command,
                    Command::Spawn { work: kobo_sdk::Task::Post { url, .. }, .. }
                        if url == "https://lichess.org/api/board/seek"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn live_seek_check_waits_for_cancelled_account_poll_to_release_its_slot() {
        let mut app = ready_app();
        app.event_open = false;
        let mut runner = AppRunner::new(app);
        runner.action(action_id("play"));
        let account = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::Account).then_some(*task))
            .expect("account request");
        runner.task_outcome(
            account,
            TaskOutcome::Completed(br#"{"id":"owner123","username":"Owner"}"#.to_vec()),
        );
        let playing = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::Playing).then_some(*task))
            .expect("playing request");
        runner.task_outcome(
            playing,
            TaskOutcome::Completed(br#"{"nowPlaying":[]}"#.to_vec()),
        );
        let event = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::EventOpen).then_some(*task))
            .expect("event open");
        runner.task_outcome(event, TaskOutcome::Completed(Vec::new()));
        let retry = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::AccountRetry).then_some(*task))
            .expect("account retry");
        let mut commands = runner.action(action_id("seek-10-0"));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Cancel(task) if *task == retry)));
        assert!(!runner
            .app()
            .has_pending(|pending| matches!(pending, Pending::SeekGrace { .. })));
        commands.extend(runner.task_outcome(retry, TaskOutcome::Cancelled));
        assert!(runner
            .app()
            .has_pending(|pending| matches!(pending, Pending::SeekGrace { .. })));
        assert!(!runner
            .app()
            .has_pending(|pending| matches!(pending, Pending::AccountRetry)));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(command,
            Command::Spawn { work: kobo_sdk::Task::Post { url, .. }, .. }
                if url == "https://lichess.org/api/board/seek"))
                .count(),
            1
        );
    }

    #[test]
    fn an_empty_live_seek_check_keeps_waiting_and_cancel_stops_checks() {
        let mut runner = AppRunner::new(ready_app());
        runner.action(action_id(api::SeekPreset::Rapid10_0.action()));
        let seek = runner.app().seek_task;
        let grace = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekGrace { generation: 1 }).then_some(*task)
            })
            .expect("scheduled check");
        runner.task_outcome(grace, TaskOutcome::Completed(Vec::new()));
        let reconcile = runner
            .app()
            .tasks
            .iter()
            .find_map(|(task, pending)| {
                matches!(pending, Pending::SeekReconcile { generation: 1 }).then_some(*task)
            })
            .expect("current games request");
        runner.task_outcome(
            reconcile,
            TaskOutcome::Completed(br#"{"nowPlaying":[]}"#.to_vec()),
        );
        assert_eq!(
            runner
                .app()
                .tasks
                .values()
                .filter(|pending| matches!(pending, Pending::SeekGrace { generation: 1 }))
                .count(),
            1
        );
        assert_eq!(runner.app().route, Route::Pairing);
        assert!(runner.app().seek_waiting);
        assert_eq!(runner.app().seek_task, seek);
        runner.action(action_id("cancel-seek"));
        assert!(!runner.app().has_pending(|pending| matches!(
            pending,
            Pending::SeekGrace { .. } | Pending::SeekReconcile { .. }
        )));
        // A late response must not reopen a cancelled pairing.
        runner.task_outcome(
            reconcile,
            TaskOutcome::Completed(br#"{"nowPlaying":[]}"#.to_vec()),
        );
        assert!(runner.app().session.is_none());
    }

    #[test]
    fn ambiguous_live_matches_cancel_the_seek_without_choosing_a_game() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_seek(&mut context, api::SeekPreset::Rapid10_0);
        let seek = app.seek_task.expect("live seek");
        app.finish_seek_reconciliation(
            &mut context,
            1,
            vec![
                summary("firstGame", api::SeekPreset::Rapid10_0),
                summary("otherGame", api::SeekPreset::Rapid10_0),
            ],
        );
        assert_eq!(app.route, Route::Play);
        assert!(!app.seek_waiting);
        assert!(app.seek_task.is_none());
        assert!(app.session.is_none());
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Cancel(task) if *task == seek)));
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
        assert_eq!(app.route, Route::Game);
        assert!(!app.seek_waiting);
        assert!(app.selected_preset.is_none());
        assert!(app.seek_candidate.is_none());
        assert_eq!(
            app.session.as_ref().map(|game| game.game_id.as_str()),
            Some("newGame2")
        );
        assert_ne!(app.notice.as_deref(), Some("No match."));
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
    fn playing_snapshot_does_not_close_an_offline_game() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.start_computer_game(&mut context);
        app.handle_completed(&mut context, Pending::Playing, br#"{"nowPlaying":[]}"#);
        assert_eq!(app.route, Route::Game);
        assert!(app.local_game);
        assert_eq!(app.game.as_ref().expect("game").id, "computer");
        assert_eq!(app.session.as_ref().expect("session").game_id, "computer");
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
        app.menu_open = true;
        let screen = format!("{:?}", app.game_screen(&Context::default()));
        assert!(screen.contains("Reconnect"));
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
    fn game_finish_on_live_board_does_not_duplicate_the_result() {
        let mut app = app_with_game(&[], Color::White);
        app.notice = Some("Move sent".to_owned());
        let mut context = Context::default();
        app.handle_event(&mut context, api::Event::GameFinish("abcdEF12".to_owned()));
        assert!(app.notice.is_none());
        assert!(app.has_pending(|pending| matches!(pending, Pending::BoardNext(_))));
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
        assert!(format!("{:?}", app.play_screen()).contains("Not connected"));
        app.tasks.insert(kobo_sdk::TaskId(8), Pending::Account);
        app.on_task(
            &mut context,
            kobo_sdk::TaskId(8),
            TaskOutcome::Completed(b"COBALT-HTTP/1 429\nRetry-After: 17\n\n".to_vec()),
        );
        assert!(app.notice.as_deref().unwrap_or_default().contains("17s"));
    }

    fn unconnected() -> Lichess {
        Lichess {
            route: Route::Play,
            account: AccountState::Missing,
            ..Lichess::default()
        }
    }

    #[test]
    fn a_missing_or_rejected_token_can_be_replaced_from_account_games() {
        let mut app = unconnected();
        let missing = format!("{:?}", app.play_screen());
        assert!(missing.contains("Not connected"), "{missing}");
        assert!(missing.contains("Connect Lichess"), "{missing}");
        app.account = AccountState::Invalid;
        let rejected = format!("{:?}", app.play_screen());
        assert!(rejected.contains("Token rejected"), "{rejected}");
        assert!(rejected.contains("Connect Lichess"), "{rejected}");
        assert!(!format!("{:?}", ready_app().play_screen()).contains("Connect Lichess"));
    }

    #[test]
    fn the_connect_prompt_says_where_to_create_the_token_and_keeps_the_computer_route() {
        let mut runner = AppRunner::new(unconnected());
        let prompt = format!(
            "{:?}",
            painted(runner.action(action_id(CONNECT))).expect("connect prompt")
        );
        assert!(
            prompt.contains("lichess.org/account/oauth/token"),
            "{prompt}"
        );
        assert!(prompt.contains("board:play"), "{prompt}");
        assert!(prompt.contains("Enter account key"), "{prompt}");
        assert!(prompt.contains("Use computer"), "{prompt}");
    }

    #[test]
    fn a_token_typed_on_the_reader_is_saved_by_the_runtime_and_checked_at_once() {
        let mut runner = AppRunner::new(unconnected());
        runner.action(action_id(CONNECT));
        let asked = runner.action(action_id("credential.enter"));
        // The token is typed on the runtime's keyboard and kept by the
        // runtime. Lichess asks for it by name and never holds it.
        assert!(asked.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::TypeText { save_as: Some(target), initial, .. })
                if target.name == "lichess" && initial.is_empty()
        )));
        let confirming = runner.answer_text(kobo_sdk::TextOutcome::Saved);
        assert!(confirming.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::CheckSecrets { names }) if names == &["lichess".to_owned()]
        )));
        let saved = painted(runner.device_result(DeviceResult::Secrets {
            present: vec!["lichess".to_owned()],
        }))
        .expect("account games");

        let app = runner.app_mut();
        assert!(!app.credential.is_open());
        assert_eq!(app.route, Route::Play);
        assert!(matches!(app.account, AccountState::Checking));
        assert!(app.has_pending(|pending| matches!(pending, Pending::Account)));
        assert!(format!("{saved:?}").contains("Checking account"));
    }

    #[test]
    fn back_from_the_connect_prompt_returns_to_account_games_without_saving() {
        let mut runner = AppRunner::new(unconnected());
        runner.action(action_id(CONNECT));
        let commands = runner.action(ActionId::BACK);
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Device(_))));
        let screen = format!("{:?}", painted(commands).expect("account games"));
        assert!(screen.contains("Account/Games"), "{screen}");
        assert!(screen.contains("Connect Lichess"), "{screen}");
        assert!(!runner.app_mut().credential.is_open());
        assert_eq!(runner.app_mut().route, Route::Play);
    }

    #[test]
    fn connecting_fits_every_panel_and_text_size() {
        for (name, panel) in panels() {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..panel
                };
                let mut runner = AppRunner::with_metrics(unconnected(), metrics);
                let play = runner.app_mut().play_screen().with_own_back(true);
                let prompt = painted(runner.action(action_id(CONNECT))).expect("connect prompt");
                for screen in [play, prompt] {
                    let chrome = Chrome::for_screen(&screen, false, None);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        !diagnostics.has_errors(),
                        "{name} {text_scale:?}: {:?}",
                        diagnostics.issues
                    );
                }
            }
        }
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
    fn active_board_releases_account_polling_capacity_for_moves() {
        let mut app = ready_app();
        let mut context = Context::default();
        app.schedule_account_retry(&mut context);
        let retry = app
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::AccountRetry).then_some(*task))
            .expect("account retry");
        let game = app_with_game(&[], Color::White);
        app.open_board(&mut context, game.session.clone().expect("session"));
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Cancel(task) if *task == retry)));
        assert!(!app.has_pending(|pending| matches!(pending, Pending::AccountRetry)));
        app.schedule_account_retry(&mut context);
        assert!(!app.has_pending(|pending| matches!(pending, Pending::AccountRetry)));
        app.clear_session(&mut context);
        app.schedule_account_retry(&mut Context::default());
        assert!(app.has_pending(|pending| matches!(pending, Pending::AccountRetry)));
    }

    #[test]
    fn valid_credential_is_rechecked_without_hiding_ready_state() {
        let mut app = ready_app();
        let mut context = Context::default();
        let account = kobo_sdk::TaskId(7);
        app.tasks.insert(account, Pending::Account);

        app.on_task(
            &mut context,
            account,
            TaskOutcome::Completed(br#"{"id":"owner123","username":"Owner"}"#.to_vec()),
        );

        let retry = app
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::AccountRetry).then_some(*task))
            .expect("credential recheck");
        app.on_task(&mut context, retry, TaskOutcome::Completed(Vec::new()));

        assert!(matches!(app.account, AccountState::Ready(_)));
        let account = app
            .tasks
            .iter()
            .find_map(|(task, pending)| matches!(pending, Pending::Account).then_some(*task))
            .expect("account probe");
        app.on_task(
            &mut context,
            account,
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
        );

        assert!(matches!(app.account, AccountState::Missing));
        assert!(format!("{:?}", app.play_screen()).contains("Not connected"));
    }

    #[test]
    fn post_opening_actions_hide_abort_and_keep_resign_and_draw_controls() {
        let mut app = app_with_game(&["e2e4", "e7e5"], Color::White);
        app.menu_open = true;
        let rendered = format!("{:?}", app.game_screen(&Context::default()));
        assert!(!rendered.contains("Abort"));
        assert!(rendered.contains("Resign"));
        assert!(rendered.contains("Offer draw"));
    }

    #[test]
    fn draw_acceptance_and_decline_wait_for_authoritative_server_state() {
        let mut accepting = app_with_game(&["e2e4", "e7e5"], Color::White);
        accepting.game.as_mut().expect("game").state.black_draw = true;
        accepting.menu_open = true;
        let offered = format!("{:?}", accepting.game_screen(&Context::default()));
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
        assert!(format!("{:?}", accepting.game_screen(&Context::default()))
            .contains("Waiting for Lichess."));
        let accepted = api::parse_board(
            br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"draw"}"#,
            "abcdEF12",
        )
        .expect("accepted draw");
        accepting.handle_board(&mut accept_context, "abcdEF12", accepted);
        assert!(format!("{:?}", accepting.game_screen(&Context::default())).contains("Draw agreed"));

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
        declining.menu_open = true;
        let cleared = format!("{:?}", declining.game_screen(&Context::default()));
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
        assert_eq!(app.route, Route::Game);
        assert!(app.seek_candidate.is_none());
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
            .contains("Refreshing the position"));

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

    #[test]
    fn flag_fall_ends_online_and_computer_games_with_a_result_dialog() {
        for local in [false, true] {
            let mut app = app_with_game(&[], Color::White);
            app.local_game = local;
            app.game.as_mut().expect("game").state.white_ms = 1_000;
            app.selected = Some("e2".to_owned());
            app.result_dismissed = true;
            let mut context = Context::default();

            assert!(app.finish_expired_clock(&mut context, 1_000));

            let game = app.game.as_ref().expect("finished game");
            assert_eq!(game.state.status, "outoftime");
            assert_eq!(game.state.winner, Some(Color::Black));
            assert!(!game.active());
            assert_eq!(app.selected, None);
            assert!(!app.result_dismissed);
            assert!(
                format!("{:?}", app.game_screen(&Context::default())).contains("Black won by time")
            );
        }
    }
}
