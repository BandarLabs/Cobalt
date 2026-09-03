//! An unofficial Lichess client.  Credentials are only named in tasks; they
//! are resolved and attached by Cobalt, so this process never reads a token.

mod chess;

use kobo_json::Value;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, Screen,
    ScreenBuilder, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const BATCH: &str = "puzzle-batch";
const LOCAL_RATING: &str = "local-puzzle-rating";
const PUZZLE_URL: &str = "https://lichess.org/api/puzzle/batch/mix?nb=32&difficulty=normal";
const PLAYING_URL: &str = "https://lichess.org/api/account/playing";
const SEEK_URL: &str = "https://lichess.org/api/board/seek";
const MAX_JSON: u32 = 250 * 1024;
const PREFETCH_AT: usize = 4;
const EMPTY_BOARD: &str = "8/8/8/8/8/8/8/8 w - - 0 1";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Home,
    Puzzles,
    Solve,
    Result,
    Games,
    Game,
    Challenge,
}

#[derive(Clone, Debug, Default)]
struct Puzzle {
    fen: String,
    solution: Vec<String>,
    cursor: usize,
    rating: i32,
}

#[derive(Default)]
struct Lichess {
    route: Route,
    batch: Vec<Puzzle>,
    queued: Vec<Puzzle>,
    current: usize,
    selected: Option<String>,
    wrong: u8,
    notice: Option<String>,
    task: Option<(TaskId, &'static str)>,
    account_attempted: bool,
    local_rating: Option<i32>,
    last_delta: Option<i32>,
    rated_current: bool,
    reveal_move: Option<String>,
    reveal_fen: Option<String>,
    games: Vec<(String, String, bool, String)>,
    seek_task: Option<TaskId>,
    seeking: Option<String>,
}

impl Lichess {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Home));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Home => self.home(),
            Route::Puzzles => self.puzzles(),
            Route::Solve => self.solve(),
            Route::Result => self.result(),
            Route::Games => self.games(),
            Route::Game => Self::game(),
            Route::Challenge => self.pairing(),
        }
    }

    fn home(&self) -> Screen {
        ScreenBuilder::new("lichess-home")
            .top_bar("Lichess")
            .tiles([
                (
                    "puzzles",
                    format!("Puzzles\n{} ready", self.remaining()),
                    Glyph::Grid,
                ),
                (
                    "play",
                    format!("Play\n{} your move", self.your_moves()),
                    Glyph::Play,
                ),
            ])
            .section("Today")
            .rows([(
                "daily",
                "Daily puzzle",
                "Download when you open it",
                Glyph::Grid,
            )])
            .section("Settings")
            .rows([("settings", "Puzzle difficulty", "Normal", Glyph::Settings)])
            .build()
    }

    fn puzzles(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-puzzles")
            .top_bar("Puzzles")
            .facts([
                ("Puzzles ready", self.remaining().to_string()),
                ("Play", "Endless".to_owned()),
                (
                    "Practice rating",
                    self.local_rating.unwrap_or(1500).to_string(),
                ),
                ("Solved today", "0".to_owned()),
                ("Streak", "0 days".to_owned()),
            ]);
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
            .primary_button("new-session", "Get new puzzles")
            .button("solve", "Keep playing")
            .button("weaknesses", "Train weaknesses")
            .build()
    }

    fn solve(&self) -> Screen {
        let Some(puzzle) = self.batch.get(self.current) else {
            return ScreenBuilder::new("lichess-solve")
                .top_bar("Solve")
                .splash(
                    Some(Glyph::Grid),
                    "No puzzles ready",
                    "Connect to Wi-Fi and get new puzzles.",
                )
                .build();
        };
        let mut screen = ScreenBuilder::new("lichess-solve")
            .top_bar("Solve")
            .secondary(format!(
                "{} to move",
                if puzzle.fen.split_whitespace().nth(1) == Some("w") {
                    "White"
                } else {
                    "Black"
                }
            ));
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
            .board(8, board_cells(&puzzle.fen, self.selected.as_deref()))
            .secondary(self.selected.as_ref().map_or_else(
                || "Tap a piece, then its destination.".to_owned(),
                |square| format!("{square} selected. Tap its destination."),
            ))
            .button("skip", "Skip puzzle")
            .build()
    }

    fn result(&self) -> Screen {
        let puzzle_rating = self
            .batch
            .get(self.current)
            .map_or(1500, |puzzle| puzzle.rating);
        let local = self.local_rating.unwrap_or(1500);
        let local_label = self
            .last_delta
            .map_or_else(|| local.to_string(), |delta| format!("{local} ({delta:+})"));
        if self.wrong > 1 {
            let answer = self.reveal_move.as_deref().unwrap_or_default();
            let origin = answer.get(..2);
            let fen = self
                .reveal_fen
                .as_deref()
                .or_else(|| {
                    self.batch
                        .get(self.current)
                        .map(|puzzle| puzzle.fen.as_str())
                })
                .unwrap_or(EMPTY_BOARD);
            return ScreenBuilder::new("lichess-result")
                .top_bar("Puzzle result")
                .secondary(format!("Correct move: {}", format_move(answer)))
                .board(8, board_cells_named(fen, origin, "answer-square"))
                .secondary(format!("Puzzle {puzzle_rating} · Practice {local_label}"))
                .primary_button("next", "Next puzzle")
                .build();
        }
        ScreenBuilder::new("lichess-result")
            .top_bar("Puzzle result")
            .heading("Solved")
            .text("Your practice rating was updated.")
            .facts([
                ("Puzzle rating", puzzle_rating.to_string()),
                ("Practice rating", local_label),
            ])
            .primary_button("next", "Next puzzle")
            .build()
    }

    fn games(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-games").top_bar("Play");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        if self.games.is_empty() {
            screen = screen.splash(
                Some(Glyph::Play),
                "No active games",
                "Start one with Quick pairing.",
            );
        } else {
            screen =
                screen
                    .section("Ongoing games")
                    .rows(self.games.iter().enumerate().map(
                        |(index, (_, opponent, mine, last))| {
                            (
                                format!("game-{index}"),
                                opponent.clone(),
                                format!(
                                    "{} · {last}",
                                    if *mine { "your move" } else { "their move" }
                                ),
                                Glyph::Grid,
                            )
                        },
                    ));
        }
        screen
            .button("refresh-games", "Refresh games")
            .button("new-challenge", "Quick pairing")
            .build()
    }

    fn pairing(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-pairing")
            .top_bar("Quick pairing")
            .text("Rated · Random color");
        if let Some(control) = &self.seeking {
            return screen
                .banner(
                    BannerLevel::Attention,
                    format!("Finding a {control} opponent…"),
                )
                .text("Keep this screen open while Lichess pairs the game.")
                .primary_button("cancel-pairing", "Cancel search")
                .build();
        }
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
            .tiles([
                ("pair-10-0", "10 + 0\nRapid", Glyph::Clock),
                ("pair-10-5", "10 + 5\nRapid", Glyph::Clock),
                ("pair-15-10", "15 + 10\nRapid", Glyph::Clock),
                ("pair-30-0", "30 + 0\nClassical", Glyph::Clock),
                ("pair-30-20", "30 + 20\nClassical", Glyph::Clock),
            ])
            .build()
    }

    fn game() -> Screen {
        ScreenBuilder::new("lichess-game")
            .top_bar("Game")
            .splash(Some(Glyph::Grid), "Choose a game", "Open one from Play.")
            .build()
    }

    fn remaining(&self) -> usize {
        self.batch.len().saturating_sub(self.current)
    }

    fn your_moves(&self) -> usize {
        self.games.iter().filter(|game| game.2).count()
    }

    fn fetch_batch(
        &mut self,
        context: &mut Context,
        credential: Option<Credential>,
        purpose: &'static str,
    ) {
        if self.task.is_some() {
            return;
        }
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: PUZZLE_URL.to_owned(),
            offset: 0,
            max_bytes: MAX_JSON,
            credential,
            headers: Vec::new(),
        }) {
            self.task = Some((task, purpose));
            if purpose != "prefetch" {
                self.notice = Some("Getting more puzzles…".to_owned());
            }
        }
    }

    fn maybe_prefetch(&mut self, context: &mut Context) {
        if self.remaining() <= PREFETCH_AT && self.queued.is_empty() && self.task.is_none() {
            self.fetch_batch(context, None, "prefetch");
        }
    }

    fn next_puzzle(&mut self, context: &mut Context) {
        self.current = self.current.saturating_add(1);
        self.wrong = 0;
        self.selected = None;
        self.last_delta = None;
        self.rated_current = false;
        self.reveal_move = None;
        self.reveal_fen = None;
        if self.current < self.batch.len() {
            self.route = Route::Solve;
            self.notice = None;
            self.maybe_prefetch(context);
        } else if self.queued.is_empty() {
            self.route = Route::Puzzles;
            self.notice = Some("Getting more puzzles…".to_owned());
            self.fetch_batch(context, None, "continue");
        } else {
            self.batch = std::mem::take(&mut self.queued);
            self.current = 0;
            self.route = Route::Solve;
            self.notice = None;
            self.maybe_prefetch(context);
        }
    }

    fn fetch_games(&mut self, context: &mut Context) {
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: PLAYING_URL.to_owned(),
            offset: 0,
            max_bytes: MAX_JSON,
            credential: Some(Credential::bearer("lichess")),
            headers: Vec::new(),
        }) {
            self.task = Some((task, "games"));
            self.notice = Some("Checking for games…".to_owned());
        }
    }

    fn start_seek(&mut self, context: &mut Context, minutes: u16, increment: u16) {
        if self.seek_task.is_some() {
            return;
        }
        let control = format!("{minutes} + {increment}");
        self.notice = None;
        self.seeking = Some(control);
        self.seek_task = context.spawn(Task::Post {
            url: SEEK_URL.to_owned(),
            body: format!(
                "rated=true&time={minutes}&increment={increment}&variant=standard&color=random"
            ),
            content_type: "application/x-www-form-urlencoded".to_owned(),
            credential: Some(Credential::bearer("lichess")),
            headers: Vec::new(),
            max_bytes: MAX_JSON,
        });
    }

    fn choose_square(&mut self, square: String) {
        let Some(puzzle) = self.batch.get_mut(self.current) else {
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

        let uci = format!("{from}{square}");
        if !chess::legal(&puzzle.fen, &uci) {
            self.notice = Some("That move is not legal here.".to_owned());
            return;
        }
        let expected = puzzle.solution.get(puzzle.cursor);
        if expected.is_some_and(|solution| solution != &uci) {
            self.wrong = self.wrong.saturating_add(1);
            self.notice = Some(if self.wrong == 1 {
                "Not it — try again".to_owned()
            } else {
                format!(
                    "Not solved. The move was {}.",
                    expected.cloned().unwrap_or_default()
                )
            });
            if self.wrong > 1 {
                let answer = expected.cloned();
                self.reveal_fen = answer.as_deref().and_then(|move_| {
                    chess::play(&puzzle.fen, move_)
                        .map(|(fen, _)| fen)
                        .or_else(|| display_move(&puzzle.fen, move_))
                });
                self.reveal_move = answer;
                self.route = Route::Result;
            }
            return;
        }
        if let Some((fen, _san)) = chess::play(&puzzle.fen, &uci) {
            puzzle.fen = fen;
            puzzle.cursor += 1;
        }
        if puzzle.cursor >= puzzle.solution.len() {
            self.route = Route::Result;
        } else {
            self.notice = None;
        }
    }

    fn record_result(&mut self, context: &mut Context) {
        if self.rated_current {
            return;
        }
        let puzzle = self
            .batch
            .get(self.current)
            .map_or(1500, |puzzle| puzzle.rating);
        let local = self.local_rating.unwrap_or(1500);
        let delta = rating_delta(local, puzzle, self.wrong);
        let updated = (local + delta).clamp(600, 3000);
        self.local_rating = Some(updated);
        self.last_delta = Some(delta);
        self.rated_current = true;
        context
            .store()
            .save(LOCAL_RATING, updated.to_string().into_bytes());
    }

    fn reveal_solution(&mut self) {
        let Some(puzzle) = self.batch.get(self.current) else {
            return;
        };
        let Some(answer) = puzzle.solution.get(puzzle.cursor).cloned() else {
            return;
        };
        self.reveal_fen = chess::play(&puzzle.fen, &answer)
            .map(|(fen, _)| fen)
            .or_else(|| display_move(&puzzle.fen, &answer));
        self.reveal_move = Some(answer);
    }

    fn parse_batch(bytes: &[u8]) -> Option<Vec<Puzzle>> {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return None;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return None;
        };
        let items = value
            .as_array()
            .or_else(|| value.get("puzzles").and_then(Value::as_array))?;
        let puzzles = items.iter().filter_map(Puzzle::read).collect::<Vec<_>>();
        (!puzzles.is_empty()).then_some(puzzles)
    }

    fn accept_batch(&mut self, bytes: &[u8]) -> bool {
        let Some(puzzles) = Self::parse_batch(bytes) else {
            return false;
        };
        self.batch = puzzles;
        self.queued.clear();
        self.current = 0;
        self.wrong = 0;
        self.last_delta = None;
        self.rated_current = false;
        self.reveal_move = None;
        self.reveal_fen = None;
        self.notice = None;
        true
    }

    fn accept_games(&mut self, bytes: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return false;
        };
        let Some(games) = value.get("nowPlaying").and_then(Value::as_array) else {
            return false;
        };
        self.games = games
            .iter()
            .filter_map(|game| {
                let id = game.get("gameId")?.as_str()?.to_owned();
                let opponent = game
                    .get("opponent")
                    .and_then(|opponent| opponent.get("username"))
                    .and_then(Value::as_str)
                    .unwrap_or("Lichess player")
                    .to_owned();
                let mine = game
                    .get("isMyTurn")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let last = game
                    .get("lastMove")
                    .and_then(Value::as_str)
                    .filter(|move_| !move_.is_empty())
                    .map_or_else(|| "No moves yet".to_owned(), format_move);
                Some((id, opponent, mine, last))
            })
            .collect();
        true
    }
}

fn rating_delta(local: i32, puzzle: i32, wrong: u8) -> i32 {
    let challenge = (puzzle - local).clamp(-800, 800) / 50;
    match wrong {
        0 => (12 + challenge).clamp(1, 24),
        1 => (6 + challenge / 2).clamp(1, 16),
        _ => -(12 - challenge).clamp(1, 24),
    }
}

impl Puzzle {
    fn read(value: &Value) -> Option<Self> {
        let puzzle = value.get("puzzle")?;
        puzzle.get("id")?.as_str()?;
        let rating = puzzle
            .get("rating")
            .and_then(Value::as_i64)
            .and_then(|rating| i32::try_from(rating).ok())
            .unwrap_or(1500);
        let solution = puzzle
            .get("solution")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
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
            fen,
            solution,
            cursor: 0,
            rating,
        })
    }
}

fn board_cells(fen: &str, selected: Option<&str>) -> Vec<(String, String, Option<Glyph>)> {
    board_cells_named(fen, selected, "square")
}

fn board_cells_named(
    fen: &str,
    selected: Option<&str>,
    action_prefix: &str,
) -> Vec<(String, String, Option<Glyph>)> {
    let placement = fen.split_whitespace().next().unwrap_or_default();
    let mut pieces = std::collections::BTreeMap::new();
    for (rank_index, rank) in placement.split('/').enumerate() {
        let mut file = 0u8;
        for character in rank.chars() {
            if let Some(empty) = character.to_digit(10) {
                file = file.saturating_add(u8::try_from(empty).expect("FEN digit fits u8"));
            } else if file < 8 {
                let square = format!("{}{}", char::from(b'a' + file), 8 - rank_index);
                pieces.insert(square, (piece_name(character), piece_glyph(character)));
                file += 1;
            }
        }
    }
    let mut cells = Vec::with_capacity(64);
    for rank in 0..8 {
        for file in 0..8 {
            let square = format!("{}{}", char::from(b'a' + file), 8 - rank);
            let (label, glyph) = pieces.get(&square).map_or_else(
                || {
                    (
                        if selected == Some(square.as_str()) {
                            "Selected square".to_owned()
                        } else {
                            " ".to_owned()
                        },
                        selected.eq(&Some(square.as_str())).then_some(Glyph::Circle),
                    )
                },
                |(name, glyph)| (format!("{name} on {square}"), Some(*glyph)),
            );
            cells.push((format!("{action_prefix}-{square}"), label, glyph));
        }
    }
    cells
}

fn format_move(uci: &str) -> String {
    match (uci.get(..2), uci.get(2..4)) {
        (Some(from), Some(to)) => format!("{from} to {to}"),
        _ => "unavailable".to_owned(),
    }
}

fn display_move(fen: &str, uci: &str) -> Option<String> {
    let bytes = uci.as_bytes();
    if bytes.len() < 4 {
        return None;
    }
    let square = |file: u8, rank: u8| {
        if !(b'a'..=b'h').contains(&file) || !(b'1'..=b'8').contains(&rank) {
            return None;
        }
        Some((usize::from(b'8' - rank), usize::from(file - b'a')))
    };
    let (from_rank, from_file) = square(bytes[0], bytes[1])?;
    let (to_rank, to_file) = square(bytes[2], bytes[3])?;
    let mut board = vec![vec![' '; 8]; 8];
    for (rank, row) in fen.split_whitespace().next()?.split('/').enumerate() {
        let mut file = 0usize;
        for character in row.chars() {
            if let Some(empty) = character.to_digit(10) {
                file = file.saturating_add(usize::try_from(empty).ok()?);
            } else if rank < 8 && file < 8 {
                board[rank][file] = character;
                file += 1;
            }
        }
    }
    let piece = *board.get(from_rank)?.get(from_file)?;
    if piece == ' ' {
        return None;
    }
    let captured = *board.get(to_rank)?.get(to_file)?;
    board[from_rank][from_file] = ' ';
    let promoted = bytes.get(4).map_or(piece, |promotion| {
        let promotion = char::from(*promotion);
        if piece.is_ascii_uppercase() {
            promotion.to_ascii_uppercase()
        } else {
            promotion.to_ascii_lowercase()
        }
    });
    board[to_rank][to_file] = promoted;

    if matches!(piece, 'K' | 'k') && from_file.abs_diff(to_file) == 2 {
        let (rook_from, rook_to) = if to_file > from_file { (7, 5) } else { (0, 3) };
        board[to_rank][rook_to] = board[to_rank][rook_from];
        board[to_rank][rook_from] = ' ';
    } else if matches!(piece, 'P' | 'p') && from_file != to_file && captured == ' ' {
        board[from_rank][to_file] = ' ';
    }

    let placement = board
        .iter()
        .map(|row| {
            let mut encoded = String::new();
            let mut empty = 0;
            for piece in row {
                if *piece == ' ' {
                    empty += 1;
                } else {
                    if empty > 0 {
                        encoded.push_str(&empty.to_string());
                        empty = 0;
                    }
                    encoded.push(*piece);
                }
            }
            if empty > 0 {
                encoded.push_str(&empty.to_string());
            }
            encoded
        })
        .collect::<Vec<_>>()
        .join("/");
    let suffix = fen.split_once(' ').map_or("", |(_, suffix)| suffix);
    Some(if suffix.is_empty() {
        placement
    } else {
        format!("{placement} {suffix}")
    })
}

fn piece_name(piece: char) -> &'static str {
    match piece {
        'K' => "White king",
        'Q' => "White queen",
        'R' => "White rook",
        'B' => "White bishop",
        'N' => "White knight",
        'P' => "White pawn",
        'k' => "Black king",
        'q' => "Black queen",
        'r' => "Black rook",
        'b' => "Black bishop",
        'n' => "Black knight",
        'p' => "Black pawn",
        _ => "Empty square",
    }
}

fn piece_glyph(piece: char) -> Glyph {
    match piece {
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
        _ => Glyph::Circle,
    }
}

impl KoboApp for Lichess {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(BATCH);
        context.store().load(LOCAL_RATING);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == BATCH {
                if let Some(bytes) = value {
                    let _ = self.accept_batch(&bytes);
                }
                self.show(context);
            } else if key == LOCAL_RATING {
                self.local_rating = value
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .and_then(|rating| rating.parse().ok());
                self.show(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            if let Some(task) = self.seek_task.take() {
                context.cancel(task);
                self.seeking = None;
            }
            self.route = Route::Home;
        } else if action == action_id("puzzles") {
            self.route = Route::Puzzles;
        } else if action == action_id("play") {
            self.route = Route::Games;
            self.fetch_games(context);
        } else if action == action_id("new-session") {
            self.account_attempted = true;
            self.fetch_batch(context, Some(Credential::bearer("lichess")), "batch");
        } else if action == action_id("solve") && self.remaining() > 0 {
            self.notice = None;
            self.route = Route::Solve;
            self.maybe_prefetch(context);
        } else if action == action_id("next") {
            self.next_puzzle(context);
        } else if action == action_id("skip") {
            self.wrong = 2;
            self.reveal_solution();
            self.route = Route::Result;
            self.record_result(context);
        } else if action == action_id("refresh-games") {
            self.fetch_games(context);
        } else if action == action_id("new-challenge") {
            self.route = Route::Challenge;
            self.notice = None;
        } else if action == action_id("pair-10-0") {
            self.start_seek(context, 10, 0);
        } else if action == action_id("pair-10-5") {
            self.start_seek(context, 10, 5);
        } else if action == action_id("pair-15-10") {
            self.start_seek(context, 15, 10);
        } else if action == action_id("pair-30-0") {
            self.start_seek(context, 30, 0);
        } else if action == action_id("pair-30-20") {
            self.start_seek(context, 30, 20);
        } else if action == action_id("cancel-pairing") {
            if let Some(task) = self.seek_task.take() {
                context.cancel(task);
            }
            self.seeking = None;
            self.notice = Some("Pairing cancelled. No move was sent.".to_owned());
        } else if (0..self.games.len()).any(|index| action == action_id(&format!("game-{index}"))) {
            self.route = Route::Game;
        } else if let Some(square) = square_action(action) {
            self.choose_square(square);
            if self.route == Route::Result {
                self.record_result(context);
            }
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.seek_task == Some(task) {
            self.seek_task = None;
            self.seeking = None;
            self.notice = Some(match outcome {
                TaskOutcome::Completed(_) => "The paired game has ended.".to_owned(),
                TaskOutcome::Failed(TaskError::NoCredential) => {
                    "Finish Lichess setup on your computer.".to_owned()
                }
                TaskOutcome::Failed(error) => Failure::of(error).naming("lichess"),
                TaskOutcome::Cancelled => "Pairing cancelled. No move was sent.".to_owned(),
            });
            self.show(context);
            return;
        }
        let Some((waiting, purpose)) = self.task.take() else {
            return;
        };
        if task != waiting {
            return;
        }
        match (purpose, outcome) {
            ("batch", TaskOutcome::Completed(bytes)) if self.accept_batch(&bytes) => {
                context.store().save(BATCH, bytes);
                self.notice = None;
                self.route = Route::Puzzles;
            }
            (purpose @ ("prefetch" | "continue"), TaskOutcome::Completed(bytes)) => {
                if let Some(puzzles) = Self::parse_batch(&bytes) {
                    context.store().save(BATCH, bytes);
                    if self.remaining() == 0 || purpose == "continue" {
                        self.batch = puzzles;
                        self.current = 0;
                        self.route = Route::Solve;
                    } else {
                        self.queued = puzzles;
                    }
                    self.notice = None;
                } else if purpose == "continue" {
                    self.notice = Some("Couldn't load more puzzles. Try again.".to_owned());
                }
            }
            ("batch", TaskOutcome::Failed(TaskError::NotFound)) => {
                self.notice = Some(if self.remaining() > 0 {
                    "No new puzzles right now. Your saved puzzles are ready.".to_owned()
                } else {
                    "No puzzles are available right now. Try again later.".to_owned()
                });
            }
            ("batch", TaskOutcome::Failed(TaskError::NoCredential)) if self.account_attempted => {
                self.account_attempted = false;
                self.fetch_batch(context, None, "batch");
            }
            ("games", TaskOutcome::Completed(bytes)) if self.accept_games(&bytes) => {
                self.notice = None;
                self.route = Route::Games;
            }
            ("prefetch", TaskOutcome::Failed(_)) => {}
            (_, TaskOutcome::Failed(TaskError::NoCredential)) => {
                self.notice = Some("Finish Lichess setup on your computer.".to_owned());
            }
            (_, TaskOutcome::Failed(error)) => {
                self.notice = Some(Failure::of(error).naming("lichess"));
            }
            (_, TaskOutcome::Cancelled) => {
                self.notice = Some("Cancelled.".to_owned());
            }
            _ => {
                self.notice =
                    Some("Lichess couldn't read this response. Open the screen again.".to_owned());
            }
        }
        self.show(context);
    }
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
        board_cells, board_cells_named, display_move, piece_glyph, rating_delta, Lichess, Puzzle,
        Route,
    };
    use kobo_sdk::{action_id, Command, Context, KoboApp, Task, TaskError, TaskId, TaskOutcome};
    use kobo_ui::{Chrome, DisplayMetrics, Glyph, TextScale, CLARA_BW_METRICS};

    #[test]
    fn board_is_an_eight_by_eight_touch_grid() {
        let app = Lichess {
            batch: vec![Puzzle {
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
                rating: 1500,
            }],
            ..Lichess::default()
        };
        let layout = app
            .solve()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for square in ["a1", "e4", "h8"] {
            assert!(layout
                .rect_of_action(action_id(&format!("square-{square}")))
                .is_some());
        }
        let cells = board_cells(super::chess::START, None);
        assert_eq!(cells.len(), 64);
        assert!(cells.iter().all(|(_, label, _)| {
            !matches!(
                label.as_str(),
                "wK" | "wQ" | "wR" | "wB" | "wN" | "wP" | "bK" | "bQ" | "bR" | "bB" | "bN" | "bP"
            )
        }));
        assert!(cells
            .iter()
            .filter(|(_, _, glyph)| glyph.is_none())
            .all(|(_, label, _)| label == " "));
        assert_eq!(piece_glyph('Q'), Glyph::ChessWhiteQueen);
    }

    #[test]
    fn puzzle_board_keeps_touch_sized_cells_on_larger_kobos() {
        let app = Lichess {
            batch: vec![Puzzle {
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
                rating: 1500,
            }],
            ..Lichess::default()
        };
        for metrics in [
            DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
                text_scale: TextScale::Default,
            },
            DisplayMetrics {
                width: 1404,
                height: 1872,
                pixels_per_inch: 227,
                text_scale: TextScale::Default,
            },
        ] {
            let layout = app.solve().layout_with(&metrics, &Chrome::default());
            for square in ["a1", "e4", "h8"] {
                let rect = layout
                    .rect_of_action(action_id(&format!("square-{square}")))
                    .expect("board square");
                assert!(rect.width >= metrics.touch_target_minimum());
                assert!(rect.height >= metrics.touch_target_minimum());
            }
        }
    }

    #[test]
    fn an_wrong_first_move_stays_local_and_a_second_ends_the_puzzle() {
        let mut app = Lichess {
            batch: vec![Puzzle {
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
                rating: 1500,
            }],
            queued: vec![Puzzle {
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
                rating: 1550,
            }],
            route: Route::Solve,
            ..Lichess::default()
        };
        app.choose_square("d2".to_owned());
        app.choose_square("d4".to_owned());
        assert_eq!(app.wrong, 1);
        app.choose_square("d2".to_owned());
        app.choose_square("d4".to_owned());
        assert_eq!(app.route, Route::Result);
        let mut context = Context::default();
        app.on_action(&mut context, action_id("next"));
        assert_eq!(app.route, Route::Solve);
    }

    #[test]
    fn puzzle_auth_only_names_the_runtime_secret() {
        let mut app = Lichess::default();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("new-session"));
        let work = context
            .commands()
            .iter()
            .find_map(|command| match command {
                Command::Spawn { work, .. } => Some(work),
                _ => None,
            })
            .expect("puzzle request");
        let Task::Fetch { credential, .. } = work else {
            panic!("puzzles are fetched")
        };
        assert_eq!(
            credential.as_ref().map(|key| key.secret.as_str()),
            Some("lichess")
        );
    }

    #[test]
    fn quick_pairing_offers_board_api_time_controls_and_starts_a_seek() {
        let mut app = Lichess {
            route: Route::Challenge,
            ..Lichess::default()
        };
        let layout = app
            .pairing()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for control in [
            "pair-10-0",
            "pair-10-5",
            "pair-15-10",
            "pair-30-0",
            "pair-30-20",
        ] {
            assert!(layout.rect_of_action(action_id(control)).is_some());
        }

        let mut context = Context::default();
        app.on_action(&mut context, action_id("pair-10-0"));
        let work = context
            .commands()
            .iter()
            .find_map(|command| match command {
                Command::Spawn { work, .. } => Some(work),
                _ => None,
            })
            .expect("pairing request");
        let Task::Post {
            url,
            body,
            content_type,
            credential,
            ..
        } = work
        else {
            panic!("pairing uses a Board API POST")
        };
        assert_eq!(url, super::SEEK_URL);
        assert_eq!(
            body,
            "rated=true&time=10&increment=0&variant=standard&color=random"
        );
        assert_eq!(content_type, "application/x-www-form-urlencoded");
        assert_eq!(
            credential.as_ref().map(|key| key.secret.as_str()),
            Some("lichess")
        );
    }

    #[test]
    fn reads_the_current_batch_response_envelope() {
        let mut app = Lichess::default();
        let response = br#"{
            "puzzles": [{
                "game": {"pgn": "e4 e5 Nf3 Nc6"},
                "puzzle": {
                    "id": "abc12",
                    "solution": ["f3e5"],
                    "initialPly": 4
                }
            }]
        }"#;

        assert!(app.accept_batch(response));
        assert_eq!(app.batch.len(), 1);
        assert_eq!(app.batch[0].solution, ["f3e5"]);
    }

    #[test]
    fn reads_empty_and_active_playing_game_responses() {
        let mut app = Lichess::default();
        assert!(app.accept_games(br#"{"nowPlaying":[]}"#));
        assert!(app.games.is_empty());

        assert!(app.accept_games(
            br#"{"nowPlaying":[{
                "gameId":"abc123",
                "isMyTurn":true,
                "lastMove":"e2e4",
                "opponent":{"username":"Reader"}
            }]}"#
        ));
        assert_eq!(
            app.games,
            [(
                "abc123".to_owned(),
                "Reader".to_owned(),
                true,
                "e2 to e4".to_owned()
            )]
        );
    }

    #[test]
    fn an_empty_refresh_does_not_cover_a_saved_puzzle() {
        let task = TaskId(7);
        let mut app = Lichess {
            batch: vec![Puzzle {
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
                rating: 1500,
            }],
            task: Some((task, "batch")),
            route: Route::Puzzles,
            ..Lichess::default()
        };
        let mut context = Context::default();

        app.on_task(&mut context, task, TaskOutcome::Failed(TaskError::NotFound));
        assert_eq!(
            app.notice.as_deref(),
            Some("No new puzzles right now. Your saved puzzles are ready.")
        );

        app.on_action(&mut context, action_id("solve"));
        assert_eq!(app.route, Route::Solve);
        assert!(app.notice.is_none());
    }

    #[test]
    fn result_shows_a_practice_rating_without_claiming_glicko() {
        let mut app = Lichess {
            batch: vec![Puzzle {
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
                rating: 1700,
            }],
            route: Route::Solve,
            local_rating: Some(1500),
            ..Lichess::default()
        };
        let mut context = Context::default();

        app.on_action(&mut context, action_id("skip"));

        assert_eq!(app.local_rating, Some(1492));
        assert_eq!(app.last_delta, Some(-8));
        assert_eq!(app.reveal_move.as_deref(), Some("e2e4"));
        let answer = board_cells_named(
            app.reveal_fen.as_deref().expect("answer position"),
            Some("e2"),
            "answer-square",
        );
        assert!(answer.iter().any(|(action, _, glyph)| {
            action == "answer-square-e2" && *glyph == Some(Glyph::Circle)
        }));
        assert!(answer.iter().any(|(action, _, glyph)| {
            action == "answer-square-e4" && *glyph == Some(Glyph::ChessWhitePawn)
        }));
        let screen = format!("{:?}", app.result());
        assert!(screen.contains("Puzzle 1700"));
        assert!(screen.contains("Practice 1492 (-8)"));
        assert!(!screen.to_ascii_lowercase().contains("glicko"));
        assert_eq!(rating_delta(1500, 2300, 2), -1);
        assert_eq!(rating_delta(1500, 700, 0), 1);
    }

    #[test]
    fn authoritative_answer_still_draws_when_local_legality_disagrees() {
        let fen = "8/4k3/5K2/8/8/8/8/8 w - - 0 1";
        assert!(super::chess::play(fen, "f6e7").is_none());
        let shown = display_move(fen, "f6e7").expect("display position");
        let cells = board_cells_named(&shown, Some("f6"), "answer-square");
        assert!(cells.iter().any(|(action, _, glyph)| {
            action == "answer-square-f6" && *glyph == Some(Glyph::Circle)
        }));
        assert!(cells.iter().any(|(action, _, glyph)| {
            action == "answer-square-e7" && *glyph == Some(Glyph::ChessWhiteKing)
        }));
    }
}
