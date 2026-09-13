//! Two players, one panel, three in a row.
//!
//! This exists to prove a point about the SDK as much as to be a game: it is
//! written entirely against the public builders, and the board is not a board
//! primitive. It is a `grid`, which is the same thing a keypad or an on-screen
//! keyboard is. If a game needs the framework to grow a new node type, the
//! framework is not general enough yet.
//!
//! The rules are the ones people actually play at a table: whoever is holding
//! the device taps, and the mark alternates. Nought goes first.

use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

/// Where the running score and the choice of opponent are kept.
const SAVED: &str = "tictactoe-v1";

const SIZE: usize = 3;

/// The same board width, as the grid primitive wants it.
const COLUMNS: u8 = 3;
const CELLS: usize = SIZE * SIZE;
/// Every line that wins, as indices into the board.
const LINES: [[usize; SIZE]; 8] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [0, 3, 6],
    [1, 4, 7],
    [2, 5, 8],
    [0, 4, 8],
    [2, 4, 6],
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mark {
    #[default]
    Empty,
    Nought,
    Cross,
}

impl Mark {
    /// What is drawn in the square. A space rather than an empty string,
    /// because an empty cell still has to occupy its place in the grid.
    const fn label(self) -> &'static str {
        match self {
            Self::Empty => " ",
            Self::Nought => "O",
            Self::Cross => "X",
        }
    }

    /// The mark itself, drawn at three fifths of the square.
    ///
    /// The letters are still the labels, because that is what the cell is
    /// called out loud and in a test, but a board is read at a glance and a
    /// letter set at label size in the middle of a square this large is not a
    /// mark, it is a caption.
    const fn glyph(self) -> Option<Glyph> {
        match self {
            Self::Empty => None,
            Self::Nought => Some(Glyph::Circle),
            Self::Cross => Some(Glyph::Close),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "nobody",
            Self::Nought => "O",
            Self::Cross => "X",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Nought => Self::Cross,
            _ => Self::Nought,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Playing,
    Won(Mark),
    Tie,
}

/// Who is holding the other side.
///
/// Two people and one panel is the game this was written for. Solo exists
/// because one person with a Kobo and ten minutes is the commoner case, and
/// the opponent is deliberately a plain one: it takes a win, blocks a loss,
/// and otherwise plays the middle, a corner, a side. It can be beaten, which
/// is the point of playing it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    #[default]
    TwoPlayers,
    Solo,
}

impl Mode {
    const fn label(self) -> &'static str {
        match self {
            Self::TwoPlayers => "Two players",
            Self::Solo => "Against the Kobo",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::TwoPlayers => Self::Solo,
            Self::Solo => Self::TwoPlayers,
        }
    }
}

/// What this session of play has come to, which is the thing a table keeps
/// track of out loud and had nowhere to live here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Score {
    nought: u16,
    cross: u16,
    ties: u16,
}

impl Score {
    fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Won(Mark::Nought) => self.nought = self.nought.saturating_add(1),
            Outcome::Won(Mark::Cross) => self.cross = self.cross.saturating_add(1),
            Outcome::Tie => self.ties = self.ties.saturating_add(1),
            _ => {}
        }
    }

    const fn played(self) -> u16 {
        self.nought
            .saturating_add(self.cross)
            .saturating_add(self.ties)
    }
}

struct Game {
    board: [Mark; CELLS],
    turn: Mark,
    outcome: Outcome,
    help: bool,
    mode: Mode,
    score: Score,
    /// Whether this game has already been added to the score. A won board
    /// stays on the panel until somebody starts the next one, and counting it
    /// twice would be the easiest mistake here to make.
    counted: bool,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            board: [Mark::Empty; CELLS],
            // Nought first, so the first tap of a fresh game is always an O.
            turn: Mark::Nought,
            outcome: Outcome::Playing,
            help: false,
            mode: Mode::default(),
            score: Score::default(),
            counted: false,
        }
    }
}

impl Game {
    /// Applies a tap. Returns whether anything actually changed.
    ///
    /// A tap on an occupied square, or any tap after the game is over, is not
    /// a move. Silently ignoring it is deliberate: repainting an E Ink panel to
    /// say "you cannot do that" is slower and more annoying than doing nothing.
    fn play(&mut self, cell: usize) -> bool {
        if self.outcome != Outcome::Playing || cell >= CELLS || self.board[cell] != Mark::Empty {
            return false;
        }
        self.board[cell] = self.turn;
        self.outcome = self.settle();
        if self.outcome == Outcome::Playing {
            self.turn = self.turn.other();
        }
        true
    }

    /// The three squares that won it, if it was won.
    fn winning_line(&self) -> Option<[usize; SIZE]> {
        LINES.into_iter().find(|line| {
            let first = self.board[line[0]];
            first != Mark::Empty && line.iter().all(|cell| self.board[*cell] == first)
        })
    }

    /// The square the Kobo takes, playing the plainest sound strategy there
    /// is: win if it can, block if it must, then the middle, a corner, a side.
    fn reply(&self) -> Option<usize> {
        let mine = self.turn;
        for mark in [mine, mine.other()] {
            for line in LINES {
                let taken = line
                    .iter()
                    .filter(|cell| self.board[**cell] == mark)
                    .count();
                let empty = line.iter().find(|cell| self.board[**cell] == Mark::Empty);
                if taken == SIZE - 1 {
                    if let Some(cell) = empty {
                        return Some(*cell);
                    }
                }
            }
        }
        [4, 0, 2, 6, 8, 1, 3, 5, 7]
            .into_iter()
            .find(|cell| self.board[*cell] == Mark::Empty)
    }

    /// Starts the next game, keeping the score and the choice of opponent.
    fn rematch(&mut self) {
        self.board = [Mark::Empty; CELLS];
        self.turn = Mark::Nought;
        self.outcome = Outcome::Playing;
        self.counted = false;
    }

    fn settle(&self) -> Outcome {
        for line in LINES {
            let first = self.board[line[0]];
            if first != Mark::Empty && line.iter().all(|cell| self.board[*cell] == first) {
                return Outcome::Won(first);
            }
        }
        if self.board.iter().all(|cell| *cell != Mark::Empty) {
            Outcome::Tie
        } else {
            Outcome::Playing
        }
    }

    /// Whose turn it is, or how it ended, in the words of whoever is playing.
    fn status(&self) -> String {
        match (self.outcome, self.mode) {
            (Outcome::Playing, Mode::Solo) if self.turn == Mark::Nought => {
                "Your turn, playing O".to_owned()
            }
            (Outcome::Playing, Mode::Solo) => "The Kobo is playing X".to_owned(),
            (Outcome::Playing, Mode::TwoPlayers) => format!("{} to play", self.turn.name()),
            (Outcome::Won(Mark::Nought), Mode::Solo) => "You win".to_owned(),
            (Outcome::Won(_), Mode::Solo) => "The Kobo wins".to_owned(),
            (Outcome::Won(mark), Mode::TwoPlayers) => format!("{} wins", mark.name()),
            (Outcome::Tie, _) => "A tie".to_owned(),
        }
    }

    /// The session, counted the way a table counts it.
    fn tally(&self) -> String {
        if self.score.played() == 0 {
            return "No games finished yet".to_owned();
        }
        let (first, second) = match self.mode {
            Mode::Solo => ("You", "Kobo"),
            Mode::TwoPlayers => ("O", "X"),
        };
        format!(
            "{first} {} · {second} {} · ties {}",
            self.score.nought, self.score.cross, self.score.ties
        )
    }

    fn encode(&self) -> String {
        format!(
            "{};{};{};{}",
            match self.mode {
                Mode::TwoPlayers => "two",
                Mode::Solo => "solo",
            },
            self.score.nought,
            self.score.cross,
            self.score.ties
        )
    }

    /// Takes back the session: how it is being played, and how it has gone.
    ///
    /// The board itself is deliberately not saved. A half-finished game of
    /// noughts and crosses is not something anybody comes back to; the score
    /// of the afternoon is.
    fn decode(&mut self, text: &str) {
        let fields: Vec<&str> = text.split(';').collect();
        let [mode, nought, cross, ties] = fields[..] else {
            return;
        };
        self.mode = if mode == "solo" {
            Mode::Solo
        } else {
            Mode::TwoPlayers
        };
        self.score = Score {
            nought: nought.parse().unwrap_or(0),
            cross: cross.parse().unwrap_or(0),
            ties: ties.parse().unwrap_or(0),
        };
    }
}

/// Cell names are fixed for the life of the application, so the action a square
/// carries never changes even as its mark does.
const NAMES: [&str; CELLS] = [
    "cell-0", "cell-1", "cell-2", "cell-3", "cell-4", "cell-5", "cell-6", "cell-7", "cell-8",
];

fn screen(game: &Game) -> Screen {
    if game.help {
        return ScreenBuilder::new("tictactoe-help")
            .top_bar("How to play")
            .owns_back(true)
            .heading("Make three in a row")
            .text("O goes first. Players take turns tapping an empty square.")
            .text("Win with three marks across, down or diagonally.")
            .text("If all nine squares fill without a line, the game is a tie.")
            .bottom_action("close-help", "Play")
            .build();
    }
    // The three squares that won it are marked on the board itself. A line of
    // noughts among nine cells is not obvious at a glance, and "O wins" above
    // a board that still looks live is the kind of thing somebody argues with.
    let winning = game.winning_line().unwrap_or([CELLS; SIZE]);
    let cells = NAMES
        .iter()
        .zip(game.board.iter())
        .enumerate()
        .map(|(index, (name, mark))| (*name, mark.label(), mark.glyph(), winning.contains(&index)));
    ScreenBuilder::new("tictactoe")
        .top_bar("Tic-tac-toe")
        .heading(game.status())
        .secondary(game.tally())
        .board_with_selection(COLUMNS, cells)
        .buttons([
            (
                "reset",
                if game.outcome == Outcome::Playing {
                    "Start again"
                } else {
                    "Next game"
                },
            ),
            ("mode", game.mode.other().label()),
        ])
        .action_bar([
            ("how-to-play", "How to play"),
            ("clear-score", "Clear score"),
        ])
        .build()
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SAVED);
        context.set_screen(screen(self));
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == SAVED {
                if let Some(text) = value.and_then(|bytes| String::from_utf8(bytes).ok()) {
                    self.decode(&text);
                }
                context.set_screen(screen(self));
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.help {
            if action == action_id("close-help") || action == ActionId::BACK {
                self.help = false;
                context.set_screen(screen(self));
            }
            return;
        }
        if action == action_id("how-to-play") {
            self.help = true;
            context.set_screen(screen(self));
            return;
        }
        if action == action_id("reset") {
            self.rematch();
            context.set_screen(screen(self));
            return;
        }
        if action == action_id("mode") {
            self.mode = self.mode.other();
            // A game half-played against one opponent is not a game against
            // the other, so the board starts again. The score is what the
            // afternoon has come to and stays.
            self.rematch();
            self.keep(context);
            context.set_screen(screen(self));
            return;
        }
        if action == action_id("clear-score") {
            self.score = Score::default();
            self.keep(context);
            context.set_screen(screen(self));
            return;
        }
        let Some(cell) = NAMES.iter().position(|name| action == action_id(name)) else {
            return;
        };
        // Only repaint when the board actually changed. On this panel an
        // unnecessary refresh is the most visible thing an application can do.
        if !self.play(cell) {
            return;
        }
        self.finish(context);
        if self.mode == Mode::Solo && self.outcome == Outcome::Playing {
            if let Some(reply) = self.reply() {
                self.play(reply);
                self.finish(context);
            }
        }
        context.set_screen(screen(self));
    }
}

impl Game {
    /// Counts a finished game once, and writes the session down.
    fn finish(&mut self, context: &mut Context) {
        if self.outcome == Outcome::Playing || self.counted {
            return;
        }
        self.counted = true;
        self.score.record(self.outcome);
        self.keep(context);
    }

    fn keep(&self, context: &mut Context) {
        context.store().save(SAVED, self.encode());
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("tictactoe", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tictactoe: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{screen, Game, Mark, Mode, Outcome, Score, CELLS, NAMES, SIZE};
    use kobo_sdk::action_id;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn the_first_tap_is_a_nought_and_marks_then_alternate() {
        let mut game = Game::default();
        assert!(game.play(0));
        assert_eq!(game.board[0], Mark::Nought);
        assert!(game.play(1));
        assert_eq!(game.board[1], Mark::Cross);
        assert!(game.play(2));
        assert_eq!(game.board[2], Mark::Nought);
    }

    #[test]
    fn an_occupied_square_is_not_a_move_and_does_not_pass_the_turn() {
        let mut game = Game::default();
        assert!(game.play(4));
        assert!(!game.play(4), "the same square was taken twice");
        assert_eq!(game.turn, Mark::Cross, "a rejected tap stole the turn");
    }

    #[test]
    fn every_winning_line_is_detected() {
        for line in super::LINES {
            let mut game = Game::default();
            for cell in line {
                game.board[cell] = Mark::Nought;
            }
            assert_eq!(game.settle(), Outcome::Won(Mark::Nought), "{line:?}");
        }
    }

    #[test]
    fn a_full_board_with_no_line_is_a_tie() {
        let mut game = Game::default();
        // O X O / O X X / X O O has no three in a row.
        for (cell, mark) in [
            Mark::Nought,
            Mark::Cross,
            Mark::Nought,
            Mark::Nought,
            Mark::Cross,
            Mark::Cross,
            Mark::Cross,
            Mark::Nought,
            Mark::Nought,
        ]
        .into_iter()
        .enumerate()
        {
            game.board[cell] = mark;
        }
        assert_eq!(game.settle(), Outcome::Tie);
    }

    #[test]
    fn the_game_stops_accepting_moves_once_it_is_won() {
        let mut game = Game::default();
        for cell in [0, 3, 1, 4, 2] {
            game.play(cell);
        }
        assert_eq!(game.outcome, Outcome::Won(Mark::Nought));
        assert!(!game.play(5), "a move was accepted after the game ended");
    }

    #[test]
    fn playing_again_clears_the_board_and_gives_nought_the_first_move() {
        let mut game = Game::default();
        for cell in [0, 3, 1, 4, 2] {
            game.play(cell);
        }
        game = Game::default();
        assert!(game.board.iter().all(|mark| *mark == Mark::Empty));
        assert_eq!(game.turn, Mark::Nought);
        assert_eq!(game.outcome, Outcome::Playing);
    }

    /// The board must be tappable, square by square, on the real panel.
    #[test]
    fn every_square_is_reachable_and_larger_than_a_finger() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for name in NAMES {
            let rect = layout
                .rect_of_action(action_id(name))
                .unwrap_or_else(|| panic!("{name} is not on the screen"));
            assert!(
                rect.height >= CLARA_BW_METRICS.touch_target_minimum(),
                "{name} is too small to tap: {rect:?}"
            );
            assert_eq!(rect.width, rect.height, "{name} is not square");
        }
    }

    /// A mark must not move the square it was placed in, or the next tap of a
    /// game played quickly lands somewhere else.
    #[test]
    fn the_board_does_not_move_as_it_fills() {
        let empty = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let mut game = Game::default();
        game.play(0);
        game.play(4);
        let played = screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for name in NAMES {
            assert_eq!(
                empty.rect_of_action(action_id(name)),
                played.rect_of_action(action_id(name)),
                "{name} moved once the board had marks in it"
            );
        }
        assert_eq!(
            empty.rect_of_action(action_id("reset")),
            played.rect_of_action(action_id("reset")),
            "the reset button moved"
        );
    }

    #[test]
    fn the_whole_board_fits_on_the_panel() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let reset = layout
            .rect_of_action(action_id("reset"))
            .expect("a reset button");
        assert!(
            reset.y + reset.height <= CLARA_BW_METRICS.height,
            "the reset button is off the bottom of the panel"
        );
        assert_eq!(CELLS, SIZE * SIZE);
    }

    /// The session is what a table keeps track of out loud, and a rematch
    /// keeps it: the board starts again, the count does not.
    #[test]
    fn a_finished_game_is_counted_once_and_a_rematch_keeps_the_score() {
        let mut runner = kobo_sdk::AppRunner::new(Game::default());
        runner.start();
        runner.store_result(kobo_sdk::StoreResult::Loaded {
            key: super::SAVED.into(),
            value: None,
        });
        for cell in [0, 3, 1, 4, 2] {
            runner.action(action_id(NAMES[cell]));
        }
        assert_eq!(runner.app().outcome, Outcome::Won(Mark::Nought));
        assert_eq!(runner.app().score.nought, 1);
        // The won board stays until somebody starts the next game, and taps
        // on it must not count it again.
        runner.action(action_id(NAMES[5]));
        assert_eq!(runner.app().score.nought, 1, "the game was counted twice");
        runner.action(action_id("reset"));
        assert!(runner.app().board.iter().all(|mark| *mark == Mark::Empty));
        assert_eq!(runner.app().score.nought, 1, "a rematch cleared the score");
        assert_eq!(runner.app().turn, Mark::Nought);
        assert!(
            runner.app().tally().contains("O 1"),
            "{}",
            runner.app().tally()
        );
    }

    /// The score and the choice of opponent survive closing the application;
    /// a half-played board deliberately does not.
    #[test]
    fn the_session_is_written_down_and_read_back() {
        let mut game = Game {
            mode: Mode::Solo,
            score: Score {
                nought: 3,
                cross: 2,
                ties: 1,
            },
            ..Game::default()
        };
        let written = game.encode();
        let mut reopened = Game::default();
        reopened.decode(&written);
        assert_eq!(reopened.mode, Mode::Solo);
        assert_eq!(reopened.score, game.score);
        assert!(reopened.board.iter().all(|mark| *mark == Mark::Empty));
        // Nonsense in the store leaves the session as it was rather than
        // taking the application down with it.
        game.decode("not a score");
        assert_eq!(game.score.nought, 3);
    }

    /// The Kobo takes a win when it has one, blocks one when it must, and
    /// otherwise plays the middle. It can be beaten; that is the point.
    #[test]
    fn the_kobo_takes_a_win_before_it_blocks_and_blocks_before_it_builds() {
        let mut game = Game {
            mode: Mode::Solo,
            turn: Mark::Cross,
            ..Game::default()
        };
        // X can finish the top row; O is one away on the left column.
        game.board[0] = Mark::Cross;
        game.board[1] = Mark::Cross;
        game.board[3] = Mark::Nought;
        game.board[6] = Mark::Nought;
        assert_eq!(game.reply(), Some(2), "a win in hand was not taken");

        let mut blocking = Game {
            mode: Mode::Solo,
            turn: Mark::Cross,
            ..Game::default()
        };
        blocking.board[0] = Mark::Nought;
        blocking.board[1] = Mark::Nought;
        assert_eq!(blocking.reply(), Some(2), "a loss in one was not blocked");

        let opening = Game {
            mode: Mode::Solo,
            turn: Mark::Cross,
            ..Game::default()
        };
        assert_eq!(opening.reply(), Some(4), "the middle was left empty");
    }

    /// One tap in solo play leaves the board with the Kobo's answer already on
    /// it, and never with two marks of the same kind added at once.
    #[test]
    fn a_solo_tap_is_answered_before_the_panel_is_drawn_again() {
        let mut runner = kobo_sdk::AppRunner::new(Game {
            mode: Mode::Solo,
            ..Game::default()
        });
        runner.start();
        runner.store_result(kobo_sdk::StoreResult::Loaded {
            key: super::SAVED.into(),
            value: None,
        });
        runner.action(action_id(NAMES[0]));
        let board = runner.app().board;
        assert_eq!(board[0], Mark::Nought);
        assert_eq!(
            board.iter().filter(|mark| **mark == Mark::Cross).count(),
            1,
            "the Kobo played more than one mark"
        );
        assert_eq!(
            runner.app().turn,
            Mark::Nought,
            "the turn did not come back"
        );
    }

    /// The three squares that won it are marked on the board, at every size.
    #[test]
    fn the_winning_line_is_marked_on_the_board_at_every_text_size() {
        let mut game = Game::default();
        for cell in [0, 3, 1, 4, 2] {
            game.play(cell);
        }
        assert_eq!(game.winning_line(), Some([0, 1, 2]));
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let drawn = screen(&game).layout_with(&metrics, &Chrome::measuring(true));
            let marked: Vec<bool> = NAMES
                .iter()
                .map(|name| {
                    drawn.nodes.iter().any(|node| {
                        matches!(
                            node.kind,
                            kobo_ui::LayoutKind::Cell(action, _, true) if action == action_id(name)
                        )
                    })
                })
                .collect();
            assert_eq!(
                marked,
                [true, true, true, false, false, false, false, false, false],
                "{text_scale:?}: the winning line is not the marked one"
            );
            let diagnostics = screen(&game).diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{text_scale:?}: {:?}",
                diagnostics.issues
            );
        }
    }

    #[test]
    fn how_to_play_is_short_and_reachable() {
        let mut game = Game::default();
        assert!(screen(&game)
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("how-to-play"))
            .is_some());
        game.help = true;
        assert!(screen(&game)
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }
}
