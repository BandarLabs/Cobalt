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
const PUZZLE_URL: &str = "https://lichess.org/api/puzzle/batch/mix?nb=32&difficulty=normal";
const PLAYING_URL: &str = "https://lichess.org/api/account/playing";
const MAX_JSON: u32 = 250 * 1024;

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
    id: String,
    fen: String,
    solution: Vec<String>,
    cursor: usize,
}

#[derive(Default)]
struct Lichess {
    route: Route,
    batch: Vec<Puzzle>,
    current: usize,
    selected: Option<String>,
    wrong: u8,
    notice: Option<String>,
    task: Option<(TaskId, &'static str)>,
    account_attempted: bool,
    games: Vec<(String, String, bool, String)>,
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
            Route::Game => self.game(),
            Route::Challenge => ScreenBuilder::new("lichess-challenge")
                .top_bar("New challenge")
                .heading("Correspondence challenge")
                .text("Creating an open challenge needs a Lichess key.")
                .button("create-challenge", "Create a 3-day challenge")
                .build(),
        }
    }

    fn home(&self) -> Screen {
        ScreenBuilder::new("lichess-home")
            .top_bar("Lichess")
            .tiles([
                ("puzzles", format!("Puzzles\n{} ready", self.remaining()), Glyph::Grid),
                ("play", format!("Play\n{} your move", self.your_moves()), Glyph::Play),
            ])
            .section("Today")
            .rows([("daily", "Daily puzzle", "Download when you open it", Glyph::Grid)])
            .section("Settings")
            .rows([("settings", "Puzzle difficulty", "Normal", Glyph::Settings)])
            .build()
    }

    fn puzzles(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-puzzles")
            .top_bar("Puzzles")
            .facts([
                ("Ready", self.remaining().to_string()),
                (
                    "Current",
                    self.batch
                        .get(self.current)
                        .map_or_else(|| "—".to_owned(), |puzzle| puzzle.id.clone()),
                ),
                ("Solved today", "0".to_owned()),
                ("Streak", "0 days".to_owned()),
            ]);
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
            .primary_button("new-session", "New session")
            .button("solve", "Resume puzzle")
            .button("weaknesses", "Train weaknesses")
            .build()
    }

    fn solve(&self) -> Screen {
        let Some(puzzle) = self.batch.get(self.current) else {
            return ScreenBuilder::new("lichess-solve")
                .top_bar("Solve")
                .empty_state("No puzzle is ready. Start a session on Wi-Fi.")
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
            .secondary("Tap a piece, then its destination.")
            .button("skip", "Skip puzzle")
            .build()
    }

    fn result(&self) -> Screen {
        ScreenBuilder::new("lichess-result")
            .top_bar("Puzzle result")
            .heading(if self.wrong > 1 { "Not solved" } else { "Solved" })
            .text(if self.wrong > 1 {
                self.batch
                    .get(self.current)
                    .and_then(|puzzle| puzzle.solution.first())
                    .map_or_else(|| "The solution could not be read.".to_owned(), |move_| {
                        format!("The first move was {move_}.")
                    })
            } else {
                "Counted in this device's local puzzle record."
                    .to_owned()
            })
            .primary_button("next", "Next puzzle")
            .build()
    }

    fn games(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lichess-games").top_bar("Play");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        if self.games.is_empty() {
            screen = screen.empty_state("Games appear here after Lichess returns your board.");
        } else {
            screen = screen.section("Ongoing games").rows(self.games.iter().enumerate().map(
                |(index, (_, opponent, mine, last))| {
                    (
                        format!("game-{index}"),
                        opponent.clone(),
                        format!("{} · {last}", if *mine { "your move" } else { "their move" }),
                        Glyph::Grid,
                    )
                },
            ));
        }
        screen
            .button("refresh-games", "Refresh games")
            .button("new-challenge", "New challenge")
            .build()
    }

    fn game(&self) -> Screen {
        ScreenBuilder::new("lichess-game")
            .top_bar("Game")
            .empty_state("Open a game from Play after it has been fetched.")
            .build()
    }

    fn remaining(&self) -> usize {
        self.batch.len().saturating_sub(self.current)
    }

    fn your_moves(&self) -> usize {
        self.games.iter().filter(|game| game.2).count()
    }

    fn fetch_batch(&mut self, context: &mut Context, credential: Option<Credential>) {
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: PUZZLE_URL.to_owned(),
            offset: 0,
            max_bytes: MAX_JSON,
            credential,
            headers: Vec::new(),
        }) {
            self.task = Some((task, "batch"));
            self.notice = Some("Fetching one 32-puzzle session.".to_owned());
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
            self.notice = Some("Checking ongoing games.".to_owned());
        }
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
                format!("Not solved. The move was {}.", expected.cloned().unwrap_or_default())
            });
            if self.wrong > 1 {
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

    fn accept_batch(&mut self, bytes: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return false;
        };
        let Some(items) = value.as_array() else {
            return false;
        };
        let puzzles = items.iter().filter_map(Puzzle::read).collect::<Vec<_>>();
        if puzzles.is_empty() {
            return false;
        }
        self.batch = puzzles;
        self.current = 0;
        self.wrong = 0;
        self.notice = None;
        true
    }
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
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let game = value.get("game")?;
        let initial_ply = puzzle.get("initialPly")?.as_i64()? as usize;
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
}

fn board_cells(fen: &str, selected: Option<&str>) -> Vec<(String, String, Option<Glyph>)> {
    let placement = fen.split_whitespace().next().unwrap_or_default();
    let mut pieces = std::collections::BTreeMap::new();
    for (rank_index, rank) in placement.split('/').enumerate() {
        let mut file = 0u8;
        for character in rank.chars() {
            if let Some(empty) = character.to_digit(10) {
                file = file.saturating_add(empty as u8);
            } else if file < 8 {
                let square = format!("{}{}", char::from(b'a' + file), 8 - rank_index);
                pieces.insert(square, piece_label(character).to_owned());
                file += 1;
            }
        }
    }
    let mut cells = Vec::with_capacity(64);
    for rank in 0..8 {
        for file in 0..8 {
            let square = format!("{}{}", char::from(b'a' + file), 8 - rank);
            let label = pieces.get(&square).cloned().unwrap_or_else(|| {
                if selected == Some(square.as_str()) {
                    "[]".to_owned()
                } else {
                    " ".to_owned()
                }
            });
            cells.push((format!("square-{square}"), label, None));
        }
    }
    cells
}

fn piece_label(piece: char) -> &'static str {
    match piece {
        'K' => "wK", 'Q' => "wQ", 'R' => "wR", 'B' => "wB", 'N' => "wN", 'P' => "wP",
        'k' => "bK", 'q' => "bQ", 'r' => "bR", 'b' => "bB", 'n' => "bN", 'p' => "bP",
        _ => " ",
    }
}

impl KoboApp for Lichess {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(BATCH);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == BATCH {
                if let Some(bytes) = value {
                    let _ = self.accept_batch(&bytes);
                }
                self.show(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.route = Route::Home;
        } else if action == action_id("puzzles") {
            self.route = Route::Puzzles;
        } else if action == action_id("play") {
            self.route = Route::Games;
            self.fetch_games(context);
        } else if action == action_id("new-session") {
            self.account_attempted = true;
            self.fetch_batch(context, Some(Credential::bearer("lichess")));
        } else if action == action_id("solve") && self.remaining() > 0 {
            self.route = Route::Solve;
        } else if action == action_id("next") {
            self.current = self.current.saturating_add(1);
            self.wrong = 0;
            self.route = Route::Solve;
        } else if action == action_id("skip") {
            self.wrong = 2;
            self.route = Route::Result;
        } else if action == action_id("refresh-games") {
            self.fetch_games(context);
        } else if action == action_id("new-challenge") {
            self.route = Route::Challenge;
        } else if (0..self.games.len()).any(|index| action == action_id(&format!("game-{index}"))) {
            self.route = Route::Game;
        } else if let Some(square) = square_action(action) {
            self.choose_square(square);
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let Some((waiting, purpose)) = self.task.take() else { return };
        if task != waiting { return; }
        match (purpose, outcome) {
            ("batch", TaskOutcome::Completed(bytes)) if self.accept_batch(&bytes) => {
                context.store().save(BATCH, bytes);
                self.route = Route::Puzzles;
            }
            ("batch", TaskOutcome::Failed(TaskError::NoCredential)) if self.account_attempted => {
                self.account_attempted = false;
                self.fetch_batch(context, None);
            }
            (_, TaskOutcome::Failed(TaskError::NoCredential)) => {
                self.notice = Some("Install a Lichess key with kobo secret set lichess.".to_owned());
            }
            (_, TaskOutcome::Failed(error)) => self.notice = Some(Failure::of(error).naming("lichess")),
            (_, TaskOutcome::Cancelled) => self.notice = Some("The request was cancelled.".to_owned()),
            _ => self.notice = Some("Lichess returned data this version cannot read.".to_owned()),
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
    use super::{board_cells, piece_label, Lichess, Puzzle, Route};
    use kobo_sdk::{action_id, Command, Context, KoboApp, Task};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn board_is_an_eight_by_eight_touch_grid() {
        let app = Lichess {
            batch: vec![Puzzle {
                id: "test".to_owned(),
                fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()],
                cursor: 0,
            }],
            ..Lichess::default()
        };
        let layout = app.solve().layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for square in ["a1", "e4", "h8"] {
            assert!(layout.rect_of_action(action_id(&format!("square-{square}"))).is_some());
        }
        assert_eq!(board_cells(super::chess::START, None).len(), 64);
        assert_eq!(piece_label('Q'), "wQ");
    }

    #[test]
    fn an_wrong_first_move_stays_local_and_a_second_ends_the_puzzle() {
        let mut app = Lichess {
            batch: vec![Puzzle {
                id: "test".to_owned(), fen: super::chess::START.to_owned(),
                solution: vec!["e2e4".to_owned()], cursor: 0,
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
        let work = context.commands().iter().find_map(|command| match command {
            Command::Spawn { work, .. } => Some(work),
            _ => None,
        }).expect("puzzle request");
        let Task::Fetch { credential, .. } = work else { panic!("puzzles are fetched") };
        assert_eq!(credential.as_ref().map(|key| key.secret.as_str()), Some("lichess"));
    }
}
