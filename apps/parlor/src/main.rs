//! Classic board games against the computer, or between two people passing
//! one reader, with original rule engines and a bounded computer opponent.
#![allow(
    clippy::cast_possible_truncation,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::unused_self
)]

mod engine;
mod games;

use engine::{choose_move, Game, Strength};
#[cfg(test)]
use games::QUIET_LIMIT;
use games::{Draughts, DraughtsMove, Kalah, Morris, MorrisMove, Reversi, ReversiMove, Rules};
use kobo_sdk::game::{self as chrome, Menu, Tap};
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult, Tile,
    TileShape,
};
use std::cmp::Ordering;
use std::process::ExitCode;

const SAVE: &str = "parlor-autosave-v1";
/// Shown in place of the usual note until a write lands again.
const SAVE_FAILED: &str =
    "Progress is not saved. Keep the app open while you make room on your reader; the next move tries again.";
const GAMES: [Title; 4] = [Title::Reversi, Title::Draughts, Title::Morris, Title::Kalah];

/// The two halves of the opponent choice on the setup screen.
const PLAY_COMPUTER: &str = "opponent-computer";
const PLAY_PEOPLE: &str = "opponent-people";
/// More menu entry that goes back to the shelf of games.
const CHANGE_GAME: &str = "change-game";
/// Foot verb that plays the next round of a match.
const NEXT_ROUND: &str = "next-round";
/// The setup screen's other choices, one action per value.
const SIDE_FIRST: &str = "side-first";
const SIDE_SECOND: &str = "side-second";
const ROTATE_ON: &str = "rotation-on";
const ROTATE_OFF: &str = "rotation-off";
const STRENGTHS: [(&str, Strength); 3] = [
    ("strength-casual", Strength::Casual),
    ("strength-club", Strength::Club),
    ("strength-strong", Strength::Strong),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Title {
    Reversi,
    Draughts,
    Morris,
    Kalah,
}

impl Title {
    const fn name(self) -> &'static str {
        match self {
            Self::Reversi => "Reversi",
            Self::Draughts => "Draughts",
            Self::Morris => "Nine Men's Morris",
            Self::Kalah => "Kalah 6,4",
        }
    }
    const fn key(self) -> &'static str {
        match self {
            Self::Reversi => "reversi",
            Self::Draughts => "draughts",
            Self::Morris => "morris",
            Self::Kalah => "kalah",
        }
    }
    /// The mark on the game's card.
    ///
    /// A drawn piece rather than a generic shape: a shelf of four identical
    /// circles is a shelf that has to be read word by word, and on a panel
    /// that takes half a second to repaint, reading it twice is the cost.
    const fn glyph(self) -> Glyph {
        match self {
            Self::Reversi => Glyph::BlackDisc,
            Self::Draughts => Glyph::BlackDraughtsKing,
            Self::Morris => Glyph::Mill,
            Self::Kalah => Glyph::Grid,
        }
    }

    /// Three or four words under the name on the card.
    ///
    /// What the game is in a phrase. The sentence belongs on the game's own
    /// screen, where there is room for it and where somebody is deciding
    /// whether to play rather than which to play.
    const fn shelf_caption(self) -> &'static str {
        match self {
            Self::Reversi => "Bracket and flip",
            Self::Draughts => "Captures are forced",
            Self::Morris => "Mills, then flying",
            Self::Kalah => "Sow and capture",
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Reversi => "Bracket discs in eight directions.",
            Self::Draughts => "Eight by eight. Captures are mandatory and chains are completed.",
            Self::Morris => "Place, form mills, move, fly, and remove.",
            Self::Kalah => "Sow four stones from each of six pits.",
        }
    }

    /// What two people sharing the reader call each side.
    ///
    /// Named for what is on the board, so that nobody has to be told which
    /// one they are: the colour of the pieces, or for Kalah, whose stones
    /// have no colour, the order of play. These used to be compass points,
    /// "Black / South", which is how the rules are written down and not how
    /// anybody sitting at a board talks.
    const fn side_name(self, side: i8) -> &'static str {
        match (self, side) {
            (Self::Kalah, 1) => "Player 1",
            (Self::Kalah, _) => "Player 2",
            (_, 1) => "Black",
            _ => "White",
        }
    }

    /// The "Your side" choice against the computer.
    const fn side_choice(self, side: i8) -> &'static str {
        match (self, side) {
            (Self::Kalah, 1) => "Moves first",
            (Self::Kalah, _) => "Moves second",
            (_, 1) => "Black, moves first",
            _ => "White, moves second",
        }
    }

    const fn rules(self) -> [&'static str; 3] {
        match self {
            Self::Reversi => [
                "Place a disc so it brackets one or more opposing discs.",
                "Bracketed discs flip to your side in every direction.",
                "When no moves remain, the player with more discs wins.",
            ],
            Self::Draughts => [
                "Move diagonally into an empty square; captures are mandatory.",
                "Jump over an opposing piece to capture it, continuing when another jump is open.",
                "Reach the far side to crown a king. Take every opposing piece to win.",
            ],
            Self::Morris => [
                "Place pieces on open points. Three in a line makes a mill.",
                "After placing, slide along a line; with three pieces you may fly anywhere.",
                "A mill lets you remove one opposing piece. Reduce them to two to win.",
            ],
            Self::Kalah => [
                "Your six pits are the row nearest you, and your store is at its right end. Choose a pit to sow its stones counter-clockwise, one per pit.",
                "End in your store to play again. End in an empty pit of your own to capture the stones opposite.",
                "When one side is empty, most stones in the stores wins.",
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    PassAndPlay,
    Solo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Menu,
    Setup(Title),
    /// A game's rules. `true` when they were opened from its setup screen,
    /// which is then where Back returns; otherwise Back returns to play.
    Rules(Title, bool),
    Board,
    Record,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Position {
    Reversi(Reversi),
    Draughts(Draughts),
    Morris(Morris),
    Kalah(Kalah),
}

impl Position {
    fn title(&self) -> Title {
        match self {
            Self::Reversi(_) => Title::Reversi,
            Self::Draughts(_) => Title::Draughts,
            Self::Morris(_) => Title::Morris,
            Self::Kalah(_) => Title::Kalah,
        }
    }
    fn turn(&self) -> i8 {
        match self {
            Self::Reversi(g) => g.turn,
            Self::Draughts(g) => g.turn,
            Self::Morris(g) => g.turn,
            Self::Kalah(g) => g.turn,
        }
    }
    fn terminal(&self) -> Option<i32> {
        match self {
            Self::Reversi(g) => g.terminal_score(1),
            Self::Draughts(g) => g.terminal_score(1),
            Self::Morris(g) => g.terminal_score(1),
            Self::Kalah(g) => g.terminal_score(1),
        }
    }

    /// Whether this position is partway through one move: a capture chain
    /// that must jump again, or a mill waiting for its removal.
    ///
    /// Undo goes back past these, because a player who takes back a double
    /// capture means the whole capture, not its second half.
    fn mid_move(&self) -> bool {
        match self {
            Self::Draughts(game) => game.forced.is_some(),
            Self::Morris(game) => game.removing,
            Self::Reversi(_) | Self::Kalah(_) => false,
        }
    }

    /// Whether the side to move has nothing to play and has to pass.
    fn forced_pass(&self) -> bool {
        matches!(self, Self::Reversi(game)
            if game.placements().is_empty() && game.terminal_score(1).is_none())
    }

    fn is_valid(&self) -> bool {
        match self {
            Self::Reversi(game) => {
                valid_side(game.turn) && game.board.iter().all(|piece| (-1..=1).contains(piece))
            }
            Self::Draughts(game) => {
                let expected = game.side() * game.side();
                valid_side(game.turn)
                    && game.board.len() == expected
                    && game.board.iter().all(|piece| (-2..=2).contains(piece))
                    && game
                        .forced
                        .is_none_or(|at| at < expected && game.board[at].signum() == game.turn)
                    && game
                        .captured
                        .iter()
                        .all(|&at| at < expected && game.board[at].signum() == -game.turn)
                    // A chain in progress is legal under both rulesets: the
                    // piece that has just captured must capture again, and
                    // that is what `forced` says. What differs is the list of
                    // pieces waiting to be lifted, which only the
                    // international rules keep on the board until the turn
                    // ends. Refusing a forced square on an 8x8 board threw
                    // away the game of anybody who shut the cover in the
                    // middle of a double capture.
                    && (game.rules == Rules::International || game.captured.is_empty())
                    && (game.rules == Rules::AngloAmerican
                        || game.forced.is_some() != game.captured.is_empty())
                    && {
                        let mut captured = game.captured.clone();
                        captured.sort_unstable();
                        captured.dedup();
                        captured.len() == game.captured.len()
                    }
            }
            Self::Morris(game) => {
                valid_side(game.turn)
                    && game.placed.iter().all(|&placed| placed <= 9)
                    && game.board.iter().all(|piece| (-1..=1).contains(piece))
                    && game.count(1) <= usize::from(game.placed[0])
                    && game.count(-1) <= usize::from(game.placed[1])
            }
            Self::Kalah(game) => {
                valid_side(game.turn)
                    && game.pits.iter().all(|&pit| pit <= 48)
                    && game.pits.iter().map(|&pit| u16::from(pit)).sum::<u16>() == 48
            }
        }
    }

    fn selected_is_valid(&self, selected: Option<usize>) -> bool {
        selected.is_none_or(|at| match self {
            Self::Reversi(_) | Self::Kalah(_) => false,
            Self::Draughts(game) => {
                at < game.board.len()
                    && game.board[at].signum() == game.turn
                    && game.forced.is_none_or(|forced| forced == at)
            }
            Self::Morris(game) => at < 24 && game.board[at] == game.turn,
        })
    }
}

/// One move, with the position it was played from.
#[derive(Clone, Debug)]
enum Step {
    Reversi(Reversi, ReversiMove),
    Draughts(Draughts, DraughtsMove),
    Morris(Morris, MorrisMove),
    Kalah(Kalah, usize),
}

impl Step {
    fn side(&self) -> i8 {
        match self {
            Self::Reversi(game, _) => game.turn,
            Self::Draughts(game, _) => game.turn,
            Self::Morris(game, _) => game.turn,
            Self::Kalah(game, _) => game.turn,
        }
    }

    /// The move that turned `before` into `after`.
    ///
    /// Read back from the two positions rather than kept alongside them, so
    /// that a game saved before moves were marked, or picked up again after
    /// the cover was shut, is marked the same as one that never stopped. The
    /// engines are deterministic, so exactly one legal move fits.
    fn between(before: &Position, after: &Position) -> Option<Self> {
        match (before, after) {
            (Position::Reversi(game), Position::Reversi(next)) => game
                .legal_moves()
                .into_iter()
                .find(|mv| {
                    // A move that left the other side with nothing to play
                    // is kept together with that side's pass.
                    let played = game.apply(mv);
                    played == *next || played.apply(&ReversiMove::Pass) == *next
                })
                .map(|mv| Self::Reversi(game.clone(), mv)),
            (Position::Draughts(game), Position::Draughts(next)) => {
                played(game, next).map(|mv| Self::Draughts(game.clone(), mv))
            }
            (Position::Morris(game), Position::Morris(next)) => {
                played(game, next).map(|mv| Self::Morris(game.clone(), mv))
            }
            (Position::Kalah(game), Position::Kalah(next)) => {
                played(game, next).map(|pit| Self::Kalah(game.clone(), pit))
            }
            _ => None,
        }
    }
}

fn played<G: Game + PartialEq>(game: &G, next: &G) -> Option<G::Move> {
    game.legal_moves()
        .into_iter()
        .find(|mv| game.apply(mv) == *next)
}

struct Parlor {
    view: View,
    position: Option<Position>,
    mode: Mode,
    strength: Strength,
    human_side: i8,
    rotate: bool,
    selected: Option<usize>,
    notice: String,
    save_failed: bool,
    history: Vec<Position>,
    record: Vec<String>,
    record_page: usize,
    undo_pending: bool,
    match_score: [u8; 2],
    scored: bool,
    loaded: bool,
    menu: Menu,
    /// Whether the setup screen was opened from a game in progress, by New
    /// game, so that Back returns to that game rather than to the shelf.
    setup_from_board: bool,
}

impl Default for Parlor {
    fn default() -> Self {
        Self {
            view: View::Menu,
            position: None,
            // Against the computer, because one person holding a reader is
            // who opens a game on it. Pass-and-play is the other half of the
            // same control on the setup screen.
            mode: Mode::Solo,
            strength: Strength::Club,
            human_side: 1,
            rotate: true,
            selected: None,
            notice: String::new(),
            save_failed: false,
            history: Vec::new(),
            record: Vec::new(),
            record_page: 0,
            undo_pending: false,
            match_score: [0, 0],
            scored: false,
            loaded: false,
            menu: Menu::default(),
            setup_from_board: false,
        }
    }
}

const fn valid_side(side: i8) -> bool {
    matches!(side, -1 | 1)
}

fn cell_name(at: usize) -> String {
    format!("cell-{at}")
}

/// "The computer" in the middle of a sentence.
fn mid_sentence(name: &str) -> String {
    if name == "The computer" {
        "the computer".to_owned()
    } else {
        name.to_owned()
    }
}

impl Parlor {
    #[cfg(test)]
    fn screen(&self) -> Screen {
        self.screen_for(&Context::default())
    }

    fn screen_for(&self, context: &Context) -> Screen {
        let screen = match self.view {
            View::Menu => self.menu_screen(),
            View::Setup(title) => self.setup_screen(title),
            View::Rules(title, _) => Self::rules_screen(title),
            View::Board => self.board_screen(),
            View::Record => self.record_screen(context),
        };
        screen.with_own_back(self.view != View::Menu)
    }

    /// What a side is called at the start of a sentence about it.
    fn name(&self, title: Title, side: i8) -> &'static str {
        match self.mode {
            Mode::Solo if side == self.human_side => "You",
            Mode::Solo => "The computer",
            Mode::PassAndPlay => title.side_name(side),
        }
    }

    /// What a side is called at the start of a line in the record.
    fn record_name(&self, title: Title, side: i8) -> &'static str {
        match self.mode {
            Mode::Solo if side == self.human_side => "You",
            Mode::Solo => "Computer",
            Mode::PassAndPlay => title.side_name(side),
        }
    }

    /// Some of one side's pieces, as the sentence about losing them says it.
    fn pieces_of(&self, title: Title, owner: i8, count: usize, one: &str, many: &str) -> String {
        let noun = if count == 1 { one } else { many };
        let number = if count == 1 {
            "one".to_owned()
        } else {
            count.to_string()
        };
        match self.mode {
            Mode::Solo if owner == self.human_side => format!("{number} of your {many}"),
            Mode::Solo => format!("{number} of the computer's {many}"),
            Mode::PassAndPlay if title == Title::Kalah => {
                format!("{number} of {}'s {many}", title.side_name(owner))
            }
            Mode::PassAndPlay => {
                format!("{number} {} {noun}", title.side_name(owner).to_lowercase())
            }
        }
    }

    /// The note after a move that hands the turn over.
    ///
    /// Between two people it says whose hands the reader goes to, and that
    /// the board has turned round for them when it has. The turn used to be
    /// said in the status line beside the score, where at the larger text
    /// sizes "board turned" was cut to "board…"; said here, it is read at the
    /// moment the board turns, and the status keeps room for whose move it
    /// is. Against the computer there is nothing to say: the reply is already
    /// on the board by the time the panel has repainted.
    fn handed_over(&self, title: Title, next: i8) -> String {
        match self.mode {
            Mode::PassAndPlay if self.rotate && next == -1 => format!(
                "Board turned. Pass the reader to {}.",
                title.side_name(next)
            ),
            Mode::PassAndPlay => format!("Pass the reader to {}.", title.side_name(next)),
            Mode::Solo => String::new(),
        }
    }

    /// Whether the board is drawn turned round.
    fn rotated(&self, position: &Position) -> bool {
        match self.mode {
            // Your own pieces nearest you, whichever side you chose, the way
            // anybody sits down at a board.
            Mode::Solo => self.human_side == -1,
            Mode::PassAndPlay => self.rotate && position.turn() == -1,
        }
    }

    /// The way the record names places on the board: as the player who
    /// moves first sees it, or against the computer, as you always see it.
    fn record_rotated(&self) -> bool {
        self.mode == Mode::Solo && self.human_side == -1
    }

    fn menu_screen(&self) -> Screen {
        // A shelf of four, not a list of four. Each game is a card with its
        // name on it: four rows carrying a line of description each is more
        // than a six inch panel holds at the larger text settings, where the
        // last of them was drawn through the bottom edge and the renderer
        // refused the screen outright. What each game is belongs on its own
        // screen, which is the one the card opens.
        let screen = self.menu_prefix().tile_grid(
            TileShape::Card,
            GAMES.map(|title| {
                (
                    title.key(),
                    title.name(),
                    title.glyph(),
                    move |tile: Tile| tile.with_caption(title.shelf_caption()),
                )
            }),
        );
        screen.build()
    }

    /// Everything above the shelf of games.
    fn menu_prefix(&self) -> ScreenBuilder {
        let screen = ScreenBuilder::new("parlor-menu")
            .top_bar("Parlor")
            .heading("Board games")
            .secondary("Play the computer, or pass the reader between two players.");
        if self.position.is_some() {
            // The game already in progress is the first thing a reader who put
            // the panel down wants, so it sits above the shelf rather than
            // under it, where a fourth card would have pushed it off the panel.
            screen.primary_button("resume", "Resume saved game")
        } else {
            screen
        }
    }

    /// What starting a game here would throw away, said before it happens.
    ///
    /// The setup screen is where a new game is confirmed: nothing is lost
    /// until Start game, so the line above it is the warning rather than a
    /// dialog in front of the screen that already asks.
    fn replacing(&self, title: Title) -> Option<String> {
        let position = self.position.as_ref()?;
        if position.terminal().is_some() || self.history.is_empty() {
            return None;
        }
        Some(if position.title() == title {
            "Starting replaces the game in progress.".to_owned()
        } else {
            format!(
                "Starting replaces your {} game in progress.",
                position.title().name()
            )
        })
    }

    fn setup_screen(&self, title: Title) -> Screen {
        // Both opponents side by side with the current one filled, rather
        // than a row that changes when tapped: a reader who has only ever
        // seen "Computer" there had no way of knowing there was anything else.
        let screen = ScreenBuilder::new("parlor-setup")
            .top_bar(title.name())
            .top_bar_action(chrome::RULES, chrome::RULES_LABEL)
            .secondary(title.detail())
            .section("Opponent")
            .segmented(
                usize::from(self.mode == Mode::PassAndPlay),
                [(PLAY_COMPUTER, "Computer"), (PLAY_PEOPLE, "Pass-and-play")],
            );
        // The same for every other choice here. These were rows that cycled
        // on a tap, which showed one value and gave no sign that tapping it
        // would change it, let alone to what.
        let screen = match self.mode {
            Mode::Solo => screen
                .section("Your side")
                .segmented(
                    usize::from(self.human_side == -1),
                    [
                        (SIDE_FIRST, title.side_choice(1)),
                        (SIDE_SECOND, title.side_choice(-1)),
                    ],
                )
                .section("Computer strength")
                .segmented(
                    STRENGTHS
                        .iter()
                        .position(|&(_, strength)| strength == self.strength)
                        .unwrap_or(1),
                    STRENGTHS.map(|(name, strength)| (name, strength.name())),
                ),
            Mode::PassAndPlay => screen.section("Board").segmented(
                usize::from(!self.rotate),
                [
                    (ROTATE_ON, "Turns for each player"),
                    (ROTATE_OFF, "Stays still"),
                ],
            ),
        };
        let screen = match self.replacing(title) {
            Some(warning) => screen.secondary(warning),
            None => screen,
        };
        screen.primary_button("start", "Start game").build()
    }

    fn rules_screen(title: Title) -> Screen {
        let [first, second, third] = title.rules();
        ScreenBuilder::new("parlor-rules")
            .top_bar(chrome::RULES_LABEL)
            .heading(title.name())
            .text(first)
            .text(second)
            .text(third)
            .build()
    }

    fn status(&self, position: &Position) -> String {
        let title = position.title();
        if let Some(score) = position.terminal() {
            let winner = match score.cmp(&0) {
                Ordering::Greater => 1,
                Ordering::Less => -1,
                Ordering::Equal => return "Draw".to_owned(),
            };
            return match self.mode {
                Mode::Solo if winner == self.human_side => "You win".to_owned(),
                Mode::Solo => "The computer wins".to_owned(),
                Mode::PassAndPlay => format!("{} wins", title.side_name(winner)),
            };
        }
        match self.mode {
            Mode::Solo if position.turn() == self.human_side => "Your move".to_owned(),
            Mode::Solo => "The computer's move".to_owned(),
            Mode::PassAndPlay => format!("{} to move", title.side_name(position.turn())),
        }
    }

    /// Rounds won in the match, each against the side that won them. It
    /// was "0–1" on its own, with nothing to say which number was whose.
    fn score(&self, title: Title) -> String {
        let [first, second] = self.match_score;
        match self.mode {
            Mode::Solo => {
                let (yours, computer) = if self.human_side == 1 {
                    (first, second)
                } else {
                    (second, first)
                };
                format!("You {yours} · Computer {computer}")
            }
            Mode::PassAndPlay => format!(
                "{} {first} · {} {second}",
                title.side_name(1),
                title.side_name(-1)
            ),
        }
    }

    fn board_screen(&self) -> Screen {
        let Some(position) = &self.position else {
            return self.menu_screen();
        };
        let title = position.title();
        let rotated = self.rotated(position);
        let mover = -position.turn();
        let steps = self.last_turn();
        let marks = marked_cells(&steps, mover);
        // The bar carries the game and nothing else. Whose move it is went in
        // there too, and on a six inch panel at the larger text settings the
        // bar cut it off mid-word: the one thing two people passing a reader
        // between them have to read was the first thing to go.
        let base = ScreenBuilder::new("parlor-board")
            .game_top_bar(title.name(), self.menu, [(CHANGE_GAME, "Change game")])
            .section_with_value(self.status(position), self.score(title));
        // What the other side just did, in words, beside the same move marked
        // on the board. Nobody can see the computer's tap, and a board of
        // identical discs does not say which one moved.
        //
        // A refused save takes the place of both lines rather than joining
        // them: at the largest text sizes three paragraphs above a board that
        // cannot shrink below a fingertip push Undo off the panel, and the
        // mark on the board still shows the move.
        let base = if self.save_failed {
            base.secondary(SAVE_FAILED)
        } else {
            let base = if self.notice.is_empty() {
                base
            } else {
                base.secondary(self.notice.as_str())
            };
            match self.last_line(title, mover, &steps, rotated) {
                Some(line) => base.secondary(line),
                None => base,
            }
        };
        let base = match position {
            Position::Reversi(game) => self.reversi_board(base, game, rotated, &marks),
            Position::Draughts(game) => self.draughts_board(base, game, rotated, &marks),
            Position::Morris(game) => self.morris_board(base, game, rotated, &marks),
            Position::Kalah(game) => self.kalah_board(base, game, rotated, &marks),
        };
        // Undo and the record, and nothing that leaves the game: that is the
        // chevron in the bar, the same as everywhere else on the reader. A
        // "Games" button beside these wrapped onto a second row at the larger
        // text settings and was drawn off the bottom of the panel.
        let next_round = position.terminal().is_some() && !self.match_score.contains(&2);
        let verbs = std::iter::once(("record", "Moves"))
            .chain(next_round.then_some((NEXT_ROUND, "Next round")));
        base.game_actions(true, verbs).build()
    }

    fn reversi_board(
        &self,
        base: ScreenBuilder,
        game: &Reversi,
        rotated: bool,
        marks: &[usize],
    ) -> ScreenBuilder {
        let legal = game.placements();
        base.board_with_selection(
            8,
            ordered(64, rotated).map(|at| {
                // A drawn ring rather than the dotted circle character. A
                // legal square is the one thing a reader is looking for, and
                // what it looked like depended on which glyph the typeface
                // happened to carry: the renderer draws this one, at the same
                // weight on every board in the catalogue.
                let (label, glyph) = match game.board[at] {
                    1 => ("", Some(Glyph::BlackDisc)),
                    -1 => ("", Some(Glyph::WhiteDisc)),
                    _ if legal.contains(&at) => ("Legal square", Some(Glyph::LegalPoint)),
                    _ => (" ", None),
                };
                (cell_name(at), label, glyph, marks.contains(&at))
            }),
        )
    }

    fn draughts_board(
        &self,
        base: ScreenBuilder,
        game: &Draughts,
        rotated: bool,
        marks: &[usize],
    ) -> ScreenBuilder {
        let moves = game.moves();
        // The square in hand is a state the renderer draws, not a character
        // written into the square. It used to be a white square drawn as the
        // text "□", which is whatever the typeface has and sits beside the
        // pieces rather than under them.
        base.board_with_selection(
            game.side() as u8,
            ordered(game.board.len(), rotated).map(|at| {
                let marked = self
                    .selected
                    .is_some_and(|from| moves.iter().any(|m| m.from == from && m.to == at));
                let (label, glyph) = if marked {
                    ("Legal square", Some(Glyph::LegalPoint))
                } else {
                    match game.board.get(at) {
                        Some(1) => ("", Some(Glyph::BlackDraughtsMan)),
                        Some(2) => ("", Some(Glyph::BlackDraughtsKing)),
                        Some(-1) => ("", Some(Glyph::WhiteDraughtsMan)),
                        Some(-2) => ("", Some(Glyph::WhiteDraughtsKing)),
                        _ => (" ", None),
                    }
                };
                (
                    cell_name(at),
                    label,
                    glyph,
                    self.selected == Some(at) || marks.contains(&at),
                )
            }),
        )
    }

    fn morris_board(
        &self,
        base: ScreenBuilder,
        game: &Morris,
        rotated: bool,
        marks: &[usize],
    ) -> ScreenBuilder {
        let moves = game.moves();
        let chosen = self.selected;
        base.board_with_selection(
            7,
            ordered(49, rotated).map(|grid| {
                let point = MORRIS_GRID.iter().position(|&g| g == grid);
                let (label, glyph) = point.map_or((" ", None), |at| {
                    let marked = moves.iter().any(|mv| match *mv {
                        MorrisMove::Place(to) | MorrisMove::Remove(to) => to == at,
                        MorrisMove::Slide(from, to) => self.selected == Some(from) && to == at,
                    });
                    if marked && game.board[at] == 0 {
                        ("Legal point", Some(Glyph::LegalPoint))
                    } else {
                        match game.board[at] {
                            1 => ("", Some(Glyph::BlackDisc)),
                            -1 => ("", Some(Glyph::WhiteDisc)),
                            _ => ("Open point", Some(Glyph::BoardPoint)),
                        }
                    }
                });
                (
                    cell_name(grid),
                    label,
                    glyph,
                    point.is_some_and(|at| chosen == Some(at)) || marks.contains(&grid),
                )
            }),
        )
    }

    fn kalah_board(
        &self,
        base: ScreenBuilder,
        game: &Kalah,
        rotated: bool,
        marks: &[usize],
    ) -> ScreenBuilder {
        // Each store says whose it is in its own square, so the line that
        // used to repeat both counts above the board is gone: it was one more
        // paragraph pushing the pits down, and the only place the owners were
        // named, as compass points.
        base.grid_with_selection(
            7,
            false,
            ordered(14, rotated).map(|display| {
                let pit = KALAH_DISPLAY[display];
                let label = match pit {
                    6 => format!("{}: {}", self.store_tag(1), game.pits[pit]),
                    13 => format!("{}: {}", self.store_tag(-1), game.pits[pit]),
                    _ => game.pits[pit].to_string(),
                };
                (cell_name(pit), label, marks.contains(&pit))
            }),
        )
    }

    /// The word in front of a store's count, saying whose it is.
    ///
    /// Whole words, the same ones the rest of the screen uses. The cell sets
    /// its label smaller when a word is too wide for it, which is a better
    /// trade than a letter that has to be decoded: the "S" and "N" these
    /// replaced stood for compass points nobody playing could see.
    fn store_tag(&self, side: i8) -> &'static str {
        match self.mode {
            Mode::Solo if side == self.human_side => "You",
            Mode::Solo => "Computer",
            Mode::PassAndPlay => Title::Kalah.side_name(side),
        }
    }

    /// The moves the other side has just made, oldest first.
    ///
    /// Everything since the player to move last moved: one move, or several
    /// when the other side earned an extra turn, jumped a chain, removed a
    /// piece for a mill, or played on because this side had to pass. Empty
    /// once the player to move has moved, which is what takes the mark away.
    fn last_turn(&self) -> Vec<Step> {
        let Some(position) = &self.position else {
            return Vec::new();
        };
        let mover = -position.turn();
        // Against the computer only its moves are marked. A game that ends
        // on the player's own move has nothing of the computer's to show.
        if self.mode == Mode::Solo && mover == self.human_side {
            return Vec::new();
        }
        let start = self
            .history
            .iter()
            .rposition(|entry| entry.turn() != mover && !entry.forced_pass())
            .map_or(0, |at| at + 1);
        if !self.history[start..]
            .iter()
            .any(|entry| entry.turn() == mover)
        {
            return Vec::new();
        }
        let after = self.history[start + 1..]
            .iter()
            .chain(std::iter::once(position));
        self.history[start..]
            .iter()
            .zip(after)
            .filter_map(|(before, after)| Step::between(before, after))
            .collect()
    }

    /// The other side's last turn in plain words, naming what the marks on
    /// the board show.
    fn last_line(&self, title: Title, mover: i8, steps: &[Step], rotated: bool) -> Option<String> {
        let who = self.name(title, mover);
        let other = -mover;
        let mine = steps.iter().filter(|step| step.side() == mover);
        Some(match steps.first()? {
            Step::Reversi(..) => {
                let mut placed = 0;
                let mut turned = 0;
                for step in mine {
                    if let Step::Reversi(game, ReversiMove::Place(at)) = step {
                        placed += 1;
                        turned += game.flips(*at).len();
                    }
                }
                let turned = self.pieces_of(title, other, turned, "disc", "discs");
                match placed {
                    0 => format!("{who} had no legal move, so the turn passed."),
                    1 => format!("{who} placed the marked disc and turned {turned}."),
                    _ => format!(
                        "{} had no legal move, so {} placed the {placed} marked discs and turned {turned}.",
                        self.name(title, other),
                        mid_sentence(who)
                    ),
                }
            }
            Step::Draughts(..) => {
                let jumps: Vec<_> = mine
                    .filter_map(|step| match step {
                        Step::Draughts(game, mv) => Some((game, mv)),
                        _ => None,
                    })
                    .collect();
                let (first_game, first) = jumps.first()?;
                let (last_game, last) = jumps.last()?;
                let taken = jumps.iter().filter(|(_, mv)| mv.captured.is_some()).count();
                let mut line = if taken == 0 {
                    format!("{who} moved a piece along the marked squares.")
                } else {
                    format!(
                        "{who} jumped along the marked squares and took {}.",
                        self.pieces_of(title, other, taken, "piece", "pieces")
                    )
                };
                if first_game.board[first.from].abs() == 1
                    && last_game.apply_move(last).board[last.to].abs() == 2
                {
                    line.push_str(" The piece is now a king.");
                }
                line
            }
            Step::Morris(..) => self.morris_line(title, mover, steps, rotated)?,
            Step::Kalah(..) => self.kalah_line(title, mover, steps)?,
        })
    }

    fn morris_line(
        &self,
        title: Title,
        mover: i8,
        steps: &[Step],
        rotated: bool,
    ) -> Option<String> {
        let who = self.name(title, mover);
        let mut line = String::new();
        for step in steps.iter().filter(|step| step.side() == mover) {
            let Step::Morris(_, mv) = step else {
                continue;
            };
            match *mv {
                MorrisMove::Place(to) => {
                    line = format!("{who} placed a piece on {}", morris_point(to, rotated));
                }
                MorrisMove::Slide(from, to) => {
                    line = format!(
                        "{who} moved a piece from {} to {}",
                        morris_point(from, rotated),
                        morris_point(to, rotated)
                    );
                }
                MorrisMove::Remove(at) => {
                    let removed = format!(
                        "removed {} from {}",
                        self.pieces_of(title, -mover, 1, "piece", "pieces"),
                        morris_point(at, rotated)
                    );
                    line = if line.is_empty() {
                        format!("{who} {removed}")
                    } else {
                        format!("{line}, and {removed}")
                    };
                }
            }
        }
        if line.is_empty() {
            return None;
        }
        line.push('.');
        Some(line)
    }

    fn kalah_line(&self, title: Title, mover: i8, steps: &[Step]) -> Option<String> {
        let who = self.name(title, mover);
        let mut sown = 0;
        let mut captured = 0;
        for step in steps.iter().filter(|step| step.side() == mover) {
            if let Step::Kalah(game, pit) = step {
                sown += 1;
                captured += usize::from(game.sow(*pit).1);
            }
        }
        let mut line = match sown {
            0 => return None,
            1 => format!("{who} sowed from the marked pit"),
            2 => format!("{who} sowed twice, from the marked pits"),
            n => format!("{who} sowed {n} times, from the marked pits"),
        };
        if captured > 0 {
            if sown > 1 {
                line.push(',');
            }
            line.push_str(" and captured ");
            line.push_str(&self.pieces_of(title, -mover, captured, "stone", "stones"));
        }
        line.push('.');
        Some(line)
    }

    fn record_text(&self) -> String {
        if self.record.is_empty() {
            "No moves yet.".to_owned()
        } else {
            self.record
                .iter()
                .enumerate()
                .map(|(index, description)| format!("{}. {description}", index + 1))
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }

    fn record_pages(&self, context: &Context) -> Vec<Vec<String>> {
        context.paginate(&self.record_text(), false)
    }

    fn record_screen(&self, context: &Context) -> Screen {
        let pages = self.record_pages(context);
        let page = self.record_page.min(pages.len().saturating_sub(1));
        let mut builder = ScreenBuilder::new("parlor-record").top_bar("Moves");
        for paragraph in &pages[page] {
            builder = builder.text(paragraph);
        }
        if pages.len() > 1 {
            builder = builder
                .page_turns("record-previous", "record-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len()).unwrap_or(u16::MAX),
                );
        }
        builder.build()
    }

    fn turn_record_page(&mut self, context: &Context, forward: bool) {
        let last = self.record_pages(context).len().saturating_sub(1);
        let page = self.record_page.min(last);
        self.record_page = if forward {
            (page + 1).min(last)
        } else {
            page.saturating_sub(1)
        };
    }

    fn start(&mut self, title: Title) {
        self.match_score = [0, 0];
        self.start_round(title);
    }

    fn start_round(&mut self, title: Title) {
        self.position = Some(match title {
            Title::Reversi => Position::Reversi(Reversi::default()),
            Title::Draughts => Position::Draughts(Draughts::new(Rules::AngloAmerican)),
            Title::Morris => Position::Morris(Morris::default()),
            Title::Kalah => Position::Kalah(Kalah::default()),
        });
        self.view = View::Board;
        self.setup_from_board = false;
        self.selected = None;
        self.history.clear();
        self.record.clear();
        self.undo_pending = false;
        self.scored = false;
        self.notice = match title {
            Title::Reversi => "Tap a hollow destination.".into(),
            Title::Draughts => {
                "Tap a piece, then a hollow destination. Captures are mandatory.".into()
            }
            Title::Morris => {
                "Placement: tap a hollow point. Completing a mill earns one removal.".into()
            }
            Title::Kalah => "Tap one of your six non-empty pits to sow counter-clockwise.".into(),
        };
        self.run_ai_if_needed();
        self.update_outcome();
    }

    fn push_history(&mut self, description: String) {
        if let Some(position) = &self.position {
            self.history.push(position.clone());
        }
        self.record.push(description);
        self.undo_pending = false;
    }

    fn tap_cell(&mut self, at: usize) {
        let Some(position) = self.position.clone() else {
            return;
        };
        if position.terminal().is_some() {
            self.notice = if self.match_score.contains(&2) {
                "The match is over. New game, in the menu at the top, starts another."
            } else {
                "The round is over. Next round plays another."
            }
            .into();
            return;
        }
        if self.mode == Mode::Solo && position.turn() != self.human_side {
            self.notice = "The computer is choosing a move.".into();
            return;
        }
        match position {
            Position::Reversi(game) => self.tap_reversi(game, at),
            Position::Draughts(game) => self.tap_draughts(game, at),
            Position::Morris(game) => {
                if let Some(point) = MORRIS_GRID.iter().position(|&grid| grid == at) {
                    self.tap_morris(game, point);
                } else {
                    self.notice = "Tap one of the 24 marked intersections.".into();
                }
            }
            Position::Kalah(game) => self.tap_kalah(game, at),
        }
        self.run_ai_if_needed();
        self.update_outcome();
    }

    fn update_outcome(&mut self) {
        if self.scored {
            return;
        }
        let Some(score) = self.position.as_ref().and_then(Position::terminal) else {
            return;
        };
        match score.cmp(&0) {
            Ordering::Greater => self.match_score[0] += 1,
            Ordering::Less => self.match_score[1] += 1,
            Ordering::Equal => {}
        }
        self.scored = true;
        self.notice = if self.match_score.contains(&2) {
            format!(
                "Match over, {}–{}. New game, in the menu at the top, starts another.",
                self.match_score[0], self.match_score[1]
            )
        } else {
            "The match is the best of three rounds. Next round plays on.".into()
        };
    }

    fn tap_reversi(&mut self, game: Reversi, at: usize) {
        let mv = ReversiMove::Place(at);
        if !game.legal_moves().contains(&mv) {
            self.notice =
                "That square brackets no opposing discs. Tap a hollow destination.".into();
            return;
        }
        self.push_history(reversi_entry(
            self.record_name(Title::Reversi, game.turn),
            &mv,
        ));
        let mut next = game.apply(&mv);
        if next.legal_moves() == [ReversiMove::Pass] && next.terminal_score(1).is_none() {
            let passed = next.turn;
            next = next.apply(&ReversiMove::Pass);
            self.notice = format!(
                "{} had no legal move, so the turn passed.",
                self.name(Title::Reversi, passed)
            );
        } else {
            self.notice = self.handed_over(Title::Reversi, next.turn);
        }
        self.position = Some(Position::Reversi(next));
    }

    fn tap_draughts(&mut self, game: Draughts, at: usize) {
        let moves = game.moves();
        if let Some(from) = self.selected {
            if let Some(mv) = moves.iter().find(|mv| mv.from == from && mv.to == at) {
                self.push_history(draughts_entry(
                    self.record_name(Title::Draughts, game.turn),
                    &game,
                    mv,
                ));
                let next = game.apply_move(mv);
                self.selected = next.forced;
                self.notice = if next.forced.is_some() {
                    "Capture chain: the same piece must capture again.".into()
                } else {
                    self.handed_over(Title::Draughts, next.turn)
                };
                self.position = Some(Position::Draughts(next));
                return;
            }
        }
        if moves.iter().any(|mv| mv.from == at) {
            self.selected = Some(at);
            self.notice = "Piece selected. Tap a hollow destination.".into();
        } else if moves.iter().any(|mv| mv.captured.is_some()) {
            self.notice = "Captures are mandatory. Select a piece that can capture.".into();
        } else {
            self.notice = "That piece has no legal move.".into();
        }
    }

    /// The note asking for a removal after a mill.
    fn remove_prompt(&self, side: i8) -> String {
        match self.mode {
            Mode::Solo => "Mill formed. Tap one of the computer's pieces to remove it.".into(),
            Mode::PassAndPlay => format!(
                "Mill formed. Tap a {} piece to remove it.",
                Title::Morris.side_name(-side).to_lowercase()
            ),
        }
    }

    fn tap_morris(&mut self, game: Morris, at: usize) {
        let moves = game.moves();
        let who = self.record_name(Title::Morris, game.turn);
        let frame = self.record_rotated();
        if game.removing {
            let mv = MorrisMove::Remove(at);
            if moves.contains(&mv) {
                self.push_history(morris_entry(who, &mv, frame));
                let next = game.apply_move(&mv);
                self.position = Some(Position::Morris(next.clone()));
                self.selected = None;
                self.notice = match self.mode {
                    Mode::PassAndPlay => format!(
                        "Piece removed. {}",
                        self.handed_over(Title::Morris, next.turn)
                    ),
                    Mode::Solo => String::new(),
                };
            } else {
                self.notice =
                    "Remove an opposing piece outside a mill, unless every piece is in a mill."
                        .into();
            }
            return;
        }
        if game.placed[usize::from(game.turn != 1)] < 9 {
            let mv = MorrisMove::Place(at);
            if moves.contains(&mv) {
                self.push_history(morris_entry(who, &mv, frame));
                let next = game.apply_move(&mv);
                self.notice = if next.removing {
                    self.remove_prompt(game.turn)
                } else {
                    self.handed_over(Title::Morris, next.turn)
                };
                self.position = Some(Position::Morris(next));
            } else {
                self.notice = "Place on an empty marked intersection.".into();
            }
            return;
        }
        if let Some(from) = self.selected {
            let mv = MorrisMove::Slide(from, at);
            if moves.contains(&mv) {
                self.push_history(morris_entry(who, &mv, frame));
                let next = game.apply_move(&mv);
                self.notice = if next.removing {
                    self.remove_prompt(game.turn)
                } else {
                    self.handed_over(Title::Morris, next.turn)
                };
                self.position = Some(Position::Morris(next));
                self.selected = None;
                return;
            }
        }
        if moves
            .iter()
            .any(|mv| matches!(mv, MorrisMove::Slide(from, _) if *from == at))
        {
            self.selected = Some(at);
            self.notice = if game.count(game.turn) == 3 {
                "Flying: choose any empty point."
            } else {
                "Piece selected. Choose an adjacent hollow point."
            }
            .into();
        } else {
            self.notice = "That piece cannot move.".into();
        }
    }

    fn tap_kalah(&mut self, game: Kalah, at: usize) {
        if !game.moves().contains(&at) {
            self.notice = "Choose a non-empty pit on your side. Stores cannot be selected.".into();
            return;
        }
        self.push_history(kalah_entry(self.record_name(Title::Kalah, game.turn), at));
        let next = game.apply_move(at);
        self.notice = if next.turn == game.turn && next.terminal_score(1).is_none() {
            "Last stone in your store: take the extra turn.".into()
        } else if next.terminal_score(1).is_some() {
            "One side is empty. Remaining stones moved to the stores.".into()
        } else {
            self.handed_over(Title::Kalah, next.turn)
        };
        self.position = Some(Position::Kalah(next));
    }

    /// Plays every move that is not the player's to make: a pass that is the
    /// only legal move, and against the computer, the computer's whole turn.
    ///
    /// Runs inside the tap that ended the player's move, so the reply is on
    /// the board when the panel next repaints: one repaint per move rather
    /// than a frame for each half of it, which on E Ink is a flash each.
    fn run_ai_if_needed(&mut self) {
        for _ in 0..64 {
            let Some(position) = self.position.clone() else {
                return;
            };
            if position.terminal().is_some() {
                return;
            }
            let title = position.title();
            if let Position::Reversi(game) = &position {
                if position.forced_pass() {
                    self.history.push(position.clone());
                    self.record.push(reversi_entry(
                        self.record_name(title, game.turn),
                        &ReversiMove::Pass,
                    ));
                    self.position = Some(Position::Reversi(game.apply(&ReversiMove::Pass)));
                    // Against the computer the line about its reply says this
                    // already, and saying it twice took three lines of panel.
                    self.notice = match self.mode {
                        Mode::PassAndPlay => format!(
                            "{} had no legal move, so the turn passed.",
                            self.name(title, game.turn)
                        ),
                        Mode::Solo => String::new(),
                    };
                    continue;
                }
            }
            if self.mode != Mode::Solo || position.turn() == self.human_side {
                return;
            }
            let Some((next, entry)) = self.computer_move(&position) else {
                return;
            };
            self.history.push(position);
            self.record.push(entry);
            self.position = Some(next);
            self.selected = None;
            self.undo_pending = false;
            self.notice.clear();
        }
    }

    /// The computer's choice in this position, and its line in the record.
    fn computer_move(&self, position: &Position) -> Option<(Position, String)> {
        let who = self.record_name(position.title(), position.turn());
        Some(match position {
            Position::Reversi(game) => {
                let mv = choose_move(game, self.strength)?;
                (Position::Reversi(game.apply(&mv)), reversi_entry(who, &mv))
            }
            Position::Draughts(game) => {
                let mv = choose_move(game, self.strength)?;
                let entry = draughts_entry(who, game, &mv);
                (Position::Draughts(game.apply(&mv)), entry)
            }
            Position::Morris(game) => {
                let mv = choose_move(game, self.strength)?;
                let entry = morris_entry(who, &mv, self.record_rotated());
                (Position::Morris(game.apply(&mv)), entry)
            }
            Position::Kalah(game) => {
                let pit = choose_move(game, self.strength)?;
                (Position::Kalah(game.apply(&pit)), kalah_entry(who, pit))
            }
        })
    }

    /// The position Undo goes back to, as an index into the history.
    ///
    /// The start of the last whole move: never the middle of a capture chain
    /// or a mill, and never a pass, which was the only thing on offer. Against
    /// the computer it is the start of the player's own last move, so that
    /// one Undo takes back that move and the computer's reply together; going
    /// back to the computer's turn would only have it play the same reply
    /// again.
    fn undo_target(&self) -> Option<usize> {
        self.history.iter().rposition(|entry| {
            !entry.mid_move()
                && !entry.forced_pass()
                && (self.mode == Mode::PassAndPlay || entry.turn() == self.human_side)
        })
    }

    fn undo(&mut self) {
        let Some(target) = self.undo_target() else {
            self.undo_pending = false;
            self.notice = chrome::NOTHING_TO_UNDO.into();
            return;
        };
        // Between two people a move is taken back only when the other one
        // agrees, so the first Undo asks and the second one does it. Undo
        // keeps its name and its place: the request is in the note above
        // the board, and playing on instead declines it.
        if self.mode == Mode::PassAndPlay && !self.undo_pending {
            self.undo_pending = true;
            self.notice =
                "Undo asked for. The other player taps Undo to agree, or plays on.".into();
            return;
        }
        if self.scored {
            if let Some(score) = self.position.as_ref().and_then(Position::terminal) {
                match score.cmp(&0) {
                    Ordering::Greater => {
                        self.match_score[0] = self.match_score[0].saturating_sub(1);
                    }
                    Ordering::Less => {
                        self.match_score[1] = self.match_score[1].saturating_sub(1);
                    }
                    Ordering::Equal => {}
                }
            }
            self.scored = false;
        }
        self.position = Some(self.history[target].clone());
        self.history.truncate(target);
        self.record.truncate(target);
        self.selected = None;
        self.undo_pending = false;
        self.notice = match self.mode {
            Mode::Solo => "Your last move and the computer's reply are taken back.".into(),
            Mode::PassAndPlay => "The last move is taken back.".into(),
        };
    }

    fn save(&self, context: &mut Context) {
        if let Some(position) = &self.position {
            context
                .store()
                .save(SAVE, encode(self, position).into_bytes());
        }
    }

    /// The game a screen is about, for Rules and New game.
    fn current_title(&self) -> Option<Title> {
        match self.view {
            View::Setup(title) | View::Rules(title, _) => Some(title),
            View::Board | View::Record => self.position.as_ref().map(Position::title),
            View::Menu => None,
        }
    }

    fn handle_action(&mut self, action: ActionId) {
        match self.menu.read(action) {
            Some(Tap::Menu) => return,
            Some(Tap::Undo) => {
                self.undo();
                return;
            }
            Some(Tap::Rules) => {
                if let Some(title) = self.current_title() {
                    self.view = View::Rules(title, matches!(self.view, View::Setup(_)));
                }
                return;
            }
            Some(Tap::NewGame) => {
                if let Some(title) = self.current_title() {
                    self.view = View::Setup(title);
                    self.setup_from_board = true;
                }
                return;
            }
            None => {}
        }
        if action == action_id("resume") {
            self.view = View::Board;
            self.notice = "Game resumed.".into();
            self.run_ai_if_needed();
            self.update_outcome();
        } else if let Some(title) = GAMES
            .iter()
            .find(|title| action == action_id(title.key()))
            .copied()
        {
            self.view = View::Setup(title);
            self.setup_from_board = false;
        } else if action == action_id(PLAY_COMPUTER) {
            self.mode = Mode::Solo;
        } else if action == action_id(PLAY_PEOPLE) {
            self.mode = Mode::PassAndPlay;
        } else if action == action_id(ROTATE_ON) || action == action_id(ROTATE_OFF) {
            self.rotate = action == action_id(ROTATE_ON);
        } else if action == action_id(SIDE_FIRST) || action == action_id(SIDE_SECOND) {
            self.human_side = if action == action_id(SIDE_FIRST) {
                1
            } else {
                -1
            };
        } else if let Some(&(_, strength)) =
            STRENGTHS.iter().find(|(name, _)| action == action_id(name))
        {
            self.strength = strength;
        } else if action == action_id("start") {
            if let View::Setup(title) = self.view {
                self.start(title);
            }
        } else if action == action_id("record") {
            self.record_page = 0;
            self.view = View::Record;
        } else if action == action_id("board") {
            self.view = View::Board;
        } else if action == action_id(NEXT_ROUND) {
            if let Some(position) = &self.position {
                if position.terminal().is_some() && !self.match_score.contains(&2) {
                    self.start_round(position.title());
                }
            }
        } else if action == action_id(CHANGE_GAME) {
            self.view = View::Menu;
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = match self.view {
                View::Rules(title, true) => View::Setup(title),
                View::Rules(_, false) | View::Record => View::Board,
                View::Setup(_) if self.setup_from_board && self.position.is_some() => View::Board,
                _ => View::Menu,
            };
        } else if let Some(at) = (0..100).find(|at| action == action_id(&cell_name(*at))) {
            self.tap_cell(at);
        }
    }
}

impl KoboApp for Parlor {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SAVE);
        context.set_screen(self.screen_for(context));
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match &result {
            StoreResult::Denied(_) => {
                self.save_failed = true;
                context.set_screen(self.screen_for(context));
                return;
            }
            StoreResult::Saved { .. } if self.save_failed => {
                self.save_failed = false;
                context.set_screen(self.screen_for(context));
                return;
            }
            _ => {}
        }
        if let StoreResult::Loaded { value, .. } = result {
            if !self.loaded {
                if let Some(bytes) = value {
                    if let Ok(text) = String::from_utf8(bytes) {
                        if let Some(saved) = decode(&text) {
                            *self = saved;
                            self.notice = "Saved game ready. Choose Resume saved game.".into();
                            self.view = View::Menu;
                        }
                    }
                }
                self.loaded = true;
                context.set_screen(self.screen_for(context));
            }
        }
    }
    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Record {
            self.turn_record_page(context, forward);
            context.set_screen(self.screen_for(context));
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::Record
            && (action == action_id("record-previous") || action == action_id("record-next"))
        {
            self.on_page_turn(context, action == action_id("record-next"));
            return;
        }
        self.handle_action(action);
        self.save(context);
        context.set_screen(self.screen_for(context));
    }
}

/// The cells the other side's last turn touched, in the board's own
/// numbering, so that each board can mark them with the state it already
/// draws for a chosen square.
fn marked_cells(steps: &[Step], mover: i8) -> Vec<usize> {
    let mut cells = Vec::new();
    for step in steps.iter().filter(|step| step.side() == mover) {
        match step {
            Step::Reversi(_, ReversiMove::Place(at)) | Step::Kalah(_, at) => cells.push(*at),
            Step::Reversi(_, ReversiMove::Pass) => {}
            Step::Draughts(_, mv) => {
                cells.push(mv.from);
                cells.extend(mv.captured);
                cells.push(mv.to);
            }
            Step::Morris(_, MorrisMove::Place(at) | MorrisMove::Remove(at)) => {
                cells.push(MORRIS_GRID[*at]);
            }
            Step::Morris(_, MorrisMove::Slide(from, to)) => {
                cells.push(MORRIS_GRID[*from]);
                cells.push(MORRIS_GRID[*to]);
            }
        }
    }
    cells
}

fn reversi_entry(who: &str, mv: &ReversiMove) -> String {
    match *mv {
        ReversiMove::Place(at) => format!("{who}: {}", square_name(at, 8)),
        ReversiMove::Pass => format!("{who}: no legal move, passed"),
    }
}

fn draughts_entry(who: &str, game: &Draughts, mv: &DraughtsMove) -> String {
    format!(
        "{who}: {}–{}{}",
        square_name(mv.from, game.side()),
        square_name(mv.to, game.side()),
        if mv.captured.is_some() {
            ", capturing"
        } else {
            ""
        }
    )
}

fn morris_entry(who: &str, mv: &MorrisMove, rotated: bool) -> String {
    match *mv {
        MorrisMove::Place(to) => format!("{who}: placed on {}", morris_point(to, rotated)),
        MorrisMove::Slide(from, to) => format!(
            "{who}: moved from {} to {}",
            morris_point(from, rotated),
            morris_point(to, rotated)
        ),
        MorrisMove::Remove(at) => format!("{who}: removed from {}", morris_point(at, rotated)),
    }
}

/// Pits are counted one to six from the left of the player who sows them,
/// which is how each player sees their own row.
fn kalah_entry(who: &str, pit: usize) -> String {
    format!("{who}: sowed pit {}", pit % 7 + 1)
}

/// Where a Morris point is, in words a player can find on the board.
///
/// The board carries no numbers, so "point 5" sent the reader counting.
/// Every point is a corner of one of the three squares or the middle of one
/// of its sides, and that is something anybody can see.
fn morris_point(point: usize, rotated: bool) -> String {
    let grid = MORRIS_GRID[point];
    let (mut row, mut column) = (grid / 7, grid % 7);
    if rotated {
        row = 6 - row;
        column = 6 - column;
    }
    let ring = row.min(column).min(6 - row).min(6 - column);
    let square = ["outer", "middle", "inner"][ring.min(2)];
    let vertical = if row == ring {
        Some("top")
    } else if row == 6 - ring {
        Some("bottom")
    } else {
        None
    };
    let horizontal = if column == ring {
        Some("left")
    } else if column == 6 - ring {
        Some("right")
    } else {
        None
    };
    match (vertical, horizontal) {
        (Some(vertical), Some(horizontal)) => {
            format!("the {square} square's {vertical} {horizontal} corner")
        }
        (Some(side), None) | (None, Some(side)) => {
            format!("the middle of the {square} square's {side} side")
        }
        (None, None) => format!("the {square} square"),
    }
}

fn ordered(length: usize, reverse: bool) -> impl Iterator<Item = usize> {
    let values: Vec<_> = if reverse {
        (0..length).rev().collect()
    } else {
        (0..length).collect()
    };
    values.into_iter()
}

fn square_name(at: usize, side: usize) -> String {
    format!("{}{}", (b'a' + (at % side) as u8) as char, side - at / side)
}

const MORRIS_GRID: [usize; 24] = [
    0, 3, 6, 8, 10, 12, 16, 17, 18, 21, 22, 23, 25, 26, 27, 30, 31, 32, 36, 38, 40, 42, 45, 48,
];
const KALAH_DISPLAY: [usize; 14] = [13, 12, 11, 10, 9, 8, 7, 0, 1, 2, 3, 4, 5, 6];

fn encode(app: &Parlor, position: &Position) -> String {
    let mode = u8::from(app.mode == Mode::Solo);
    let strength = match app.strength {
        Strength::Casual => 0,
        Strength::Club => 1,
        Strength::Strong => 2,
    };
    // The second field was the draughts ruleset while the ten by ten board
    // was offered. Only eight by eight is played now, and an older Parlor
    // reading this save still expects the field, so it stays, always 0.
    let header = format!(
        "{mode},0,{strength},{},{},{},{},{},{},{}",
        app.human_side,
        u8::from(app.rotate),
        position.turn(),
        app.match_score[0],
        app.match_score[1],
        u8::from(app.scored),
        app.selected.map_or_else(|| "-".into(), |at| at.to_string())
    );
    let body = match position {
        Position::Reversi(g) => format!(
            "R;{}",
            g.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Draughts(g) => format!(
            "D,{},{};{}",
            g.forced.map_or_else(|| "-".into(), |n| n.to_string()),
            encode_usizes(&g.captured),
            g.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Morris(g) => format!(
            "M,{},{},{},{};{}",
            g.placed[0],
            g.placed[1],
            u8::from(g.removing),
            g.quiet,
            g.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Kalah(g) => format!(
            "K;{}",
            g.pits
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    let history = app
        .history
        .iter()
        .map(encode_snapshot)
        .collect::<Vec<_>>()
        .join("~");
    format!("{header};{body};{};{history}", app.record.join("|"))
}

struct SavedSettings {
    mode: Mode,
    strength: Strength,
    human_side: i8,
    rotate: bool,
    turn: i8,
    match_score: [u8; 2],
    scored: bool,
    selected: Option<usize>,
}

fn decode_settings(fields: &[&str]) -> Option<SavedSettings> {
    if fields.len() != 10 {
        return None;
    }
    let mode = match *fields.first()? {
        "0" => Mode::PassAndPlay,
        "1" => Mode::Solo,
        _ => return None,
    };
    // The old draughts ruleset choice. Read and set aside: the board itself
    // says which rules a saved game was played under.
    if !matches!(*fields.get(1)?, "0" | "1") {
        return None;
    }
    let strength = match *fields.get(2)? {
        "0" => Strength::Casual,
        "1" => Strength::Club,
        "2" => Strength::Strong,
        _ => return None,
    };
    let human_side = fields.get(3)?.parse().ok()?;
    let rotate = match *fields.get(4)? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let turn = fields.get(5)?.parse().ok()?;
    let match_score = [fields.get(6)?.parse().ok()?, fields.get(7)?.parse().ok()?];
    let scored = match *fields.get(8)? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(SavedSettings {
        mode,
        strength,
        human_side,
        rotate,
        turn,
        match_score,
        scored,
        selected: parse_optional_usize(fields.get(9)?).ok()?,
    })
}

/// The draughts rules a saved board was played under, from its size.
///
/// Taken from the board rather than from the ruleset choice saved beside it,
/// because that choice was the setup screen's and could change after the
/// game began: choosing ten by ten and then not starting it wrote a save
/// that said ten by ten about an eight by eight game, and the game was
/// refused when the reader next opened it.
fn draughts_rules(board: &[i8]) -> Rules {
    if board.len() == 100 {
        Rules::International
    } else {
        Rules::AngloAmerican
    }
}

fn decode_position(kind: &[&str], board: &str, turn: i8) -> Option<Position> {
    Some(match *kind.first()? {
        "R" if kind.len() == 1 => Position::Reversi(Reversi {
            board: parse_array(board)?,
            turn,
        }),
        "D" if kind.len() == 3 => {
            let board: Vec<i8> = parse_vec(board)?;
            Position::Draughts(Draughts {
                rules: draughts_rules(&board),
                board,
                turn,
                forced: parse_optional_usize(kind.get(1)?).ok()?,
                captured: parse_usizes(kind.get(2)?)?,
            })
        }
        // Four fields is a game written down before the fifty move rule
        // existed; it reads back as a game with nothing counted yet, which is
        // the truth about it.
        "M" if matches!(kind.len(), 4 | 5) => Position::Morris(Morris {
            board: parse_array(board)?,
            turn,
            placed: [kind.get(1)?.parse().ok()?, kind.get(2)?.parse().ok()?],
            removing: match *kind.get(3)? {
                "0" => false,
                "1" => true,
                _ => return None,
            },
            quiet: match kind.get(4) {
                Some(quiet) => quiet.parse().ok()?,
                None => 0,
            },
        }),
        "K" if kind.len() == 1 => Position::Kalah(Kalah {
            pits: parse_array(board)?,
            turn,
        }),
        _ => return None,
    })
}

fn decode(text: &str) -> Option<Parlor> {
    let mut parts = text.split(';');
    let settings = decode_settings(&parts.next()?.split(',').collect::<Vec<_>>())?;
    let kind = parts.next()?.split(',').collect::<Vec<_>>();
    let position = decode_position(&kind, parts.next()?, settings.turn)?;
    let record = match parts.next()? {
        "" => Vec::new(),
        moves => moves.split('|').map(str::to_owned).collect(),
    };
    let history = match parts.next()? {
        "" => Vec::new(),
        snapshots => snapshots
            .split('~')
            .map(decode_snapshot)
            .collect::<Option<Vec<_>>>()?,
    };
    // Ten by ten draughts was never playable here: the panel draws at most
    // eighty-one cells and starting one was refused. A save holding one is
    // not a game anybody could have been playing.
    let unplayable = matches!(
        &position,
        Position::Draughts(Draughts {
            rules: Rules::International,
            ..
        })
    );
    if parts.next().is_some()
        || unplayable
        || !valid_side(settings.human_side)
        || settings.match_score.iter().any(|&score| score > 2)
        || !position.is_valid()
        || !position.selected_is_valid(settings.selected)
        || record.len() != history.len()
        || history
            .iter()
            .any(|snapshot| snapshot.title() != position.title() || !snapshot.is_valid())
    {
        return None;
    }
    Some(Parlor {
        view: View::Menu,
        position: Some(position),
        mode: settings.mode,
        strength: settings.strength,
        human_side: settings.human_side,
        rotate: settings.rotate,
        selected: settings.selected,
        notice: String::new(),
        save_failed: false,
        history,
        record,
        record_page: 0,
        undo_pending: false,
        match_score: settings.match_score,
        scored: settings.scored,
        loaded: true,
        menu: Menu::default(),
        setup_from_board: false,
    })
}

fn parse_vec<T: std::str::FromStr>(text: &str) -> Option<Vec<T>> {
    text.split(',').map(|n| n.parse().ok()).collect()
}

fn parse_array<T: std::str::FromStr, const N: usize>(text: &str) -> Option<[T; N]> {
    parse_vec(text)?.try_into().ok()
}

fn parse_optional_usize(text: &str) -> Result<Option<usize>, std::num::ParseIntError> {
    if text == "-" {
        Ok(None)
    } else {
        text.parse().map(Some)
    }
}

fn encode_usizes(values: &[usize]) -> String {
    if values.is_empty() {
        "-".into()
    } else {
        values
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn parse_usizes(text: &str) -> Option<Vec<usize>> {
    if text == "-" {
        Some(Vec::new())
    } else {
        text.split('.').map(|value| value.parse().ok()).collect()
    }
}

fn encode_snapshot(position: &Position) -> String {
    match position {
        Position::Reversi(game) => format!(
            "R:{}:{}",
            game.turn,
            game.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Draughts(game) => format!(
            "D:{}:{}:{}:{}:{}",
            u8::from(game.rules == Rules::International),
            game.turn,
            game.forced.map_or_else(|| "-".into(), |at| at.to_string()),
            encode_usizes(&game.captured),
            game.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Morris(game) => format!(
            "M:{}:{}:{}:{}:{}:{}",
            game.turn,
            game.placed[0],
            game.placed[1],
            u8::from(game.removing),
            game.quiet,
            game.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Kalah(game) => format!(
            "K:{}:{}",
            game.turn,
            game.pits
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn decode_snapshot(snapshot: &str) -> Option<Position> {
    let fields: Vec<_> = snapshot.split(':').collect();
    match *fields.first()? {
        "R" => Some(Position::Reversi(Reversi {
            turn: fields.get(1)?.parse().ok()?,
            board: parse_array(fields.get(2)?)?,
        })),
        "D" => {
            // The ruleset flag in the second field is read from the board
            // instead, for the reason `draughts_rules` gives.
            let board: Vec<i8> = parse_vec(fields.get(5)?)?;
            Some(Position::Draughts(Draughts {
                rules: draughts_rules(&board),
                turn: fields.get(2)?.parse().ok()?,
                forced: fields.get(3).and_then(|at| at.parse().ok()),
                captured: parse_usizes(fields.get(4)?)?,
                board,
            }))
        }
        // Six fields is a snapshot written before the fifty move rule
        // existed: the board is where the counter now sits, and the game it
        // describes has nothing counted yet.
        "M" => Some(Position::Morris(Morris {
            turn: fields.get(1)?.parse().ok()?,
            placed: [fields.get(2)?.parse().ok()?, fields.get(3)?.parse().ok()?],
            removing: fields.get(4)? == &"1",
            quiet: match fields.get(6) {
                Some(_) => fields.get(5)?.parse().ok()?,
                None => 0,
            },
            board: parse_array(fields.get(6).or_else(|| fields.get(5))?)?,
        })),
        "K" => Some(Position::Kalah(Kalah {
            turn: fields.get(1)?.parse().ok()?,
            pits: parse_array(fields.get(2)?)?,
        })),
        _ => None,
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("parlor", Parlor::default()).map_or_else(
        |error| {
            eprintln!("parlor: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::perft;
    use crate::games::DraughtsMove;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn reversi_opening_perft_matches_reference() {
        let game = Reversi::default();
        assert_eq!(perft(&game, 1), 4);
        assert_eq!(perft(&game, 2), 12);
        assert_eq!(perft(&game, 3), 56);
    }

    #[test]
    fn both_draughts_openings_and_mandatory_chain_are_pinned() {
        let anglo = Draughts::new(Rules::AngloAmerican);
        assert_eq!(perft(&anglo, 1), 7);
        assert_eq!(perft(&anglo, 2), 49);
        assert_eq!(perft(&anglo, 3), 302);
        let international = Draughts::new(Rules::International);
        assert_eq!(perft(&international, 1), 9);
        assert_eq!(perft(&international, 2), 81);
        assert_eq!(perft(&international, 3), 658);
        let mut game = Draughts::new(Rules::AngloAmerican);
        game.board.fill(0);
        game.board[42] = 1;
        game.board[33] = -1;
        game.board[17] = -1;
        let first = game.moves().into_iter().find(|m| m.from == 42).unwrap();
        let next = game.apply_move(&first);
        assert_eq!(next.turn, 1);
        assert_eq!(next.forced, Some(first.to));
        assert_eq!(next.board[33], 0);
        assert!(next.captured.is_empty());
        assert!(next.moves().iter().all(|m| m.from == first.to));
    }

    #[test]
    fn international_kings_fly_and_capture_over_distance() {
        let mut game = Draughts::new(Rules::International);
        game.board.fill(0);
        game.board[77] = 2;
        assert!(game.moves().iter().any(|m| m.to == 22));
        game.board[44] = -1;
        let captures = game.moves();
        assert!(captures.iter().all(|m| m.captured == Some(44)));
        assert!(captures.len() > 1);
    }

    #[test]
    fn draughts_capture_code_does_not_silently_hybridize_rules() {
        let mut anglo = Draughts::new(Rules::AngloAmerican);
        anglo.board.fill(0);
        anglo.board[17] = 1;
        anglo.board[26] = -1;
        assert!(anglo.moves().iter().all(|mv| mv.captured.is_none()));

        let mut international = Draughts::new(Rules::International);
        international.board.fill(0);
        international.board[77] = 2;
        international.board[66] = -1;
        international.board[22] = -1;
        let moves = international.moves();
        assert!(!moves.is_empty());
        assert!(moves.iter().all(|mv| international.capture_length(mv) == 2));
    }

    #[test]
    fn international_captured_piece_blocks_a_kings_return_path_until_turn_end() {
        let mut chain = Draughts::new(Rules::International);
        chain.board.fill(0);
        chain.board[77] = 2;
        chain.board[66] = -1;
        chain.board[46] = -1;
        let first = DraughtsMove {
            from: 77,
            to: 55,
            captured: Some(66),
        };
        assert!(chain.moves().contains(&first));
        let intermediate = chain.apply_move(&first);
        assert_eq!(intermediate.turn, 1);
        assert_eq!(intermediate.forced, Some(55));
        assert_eq!(intermediate.board[66], -1);
        assert_eq!(intermediate.captured, vec![66]);
        assert!(intermediate
            .moves()
            .iter()
            .all(|mv| mv.captured == Some(46)));
        let finished = intermediate.apply_move(&intermediate.moves()[0]);
        assert_eq!(finished.turn, -1);
        assert_eq!(finished.board[66], 0);
        assert_eq!(finished.board[46], 0);
        assert!(finished.captured.is_empty());

        let mut game = Draughts::new(Rules::International);
        game.board.fill(0);
        game.board[77] = 2;
        game.board[66] = -1;
        game.board[88] = -1;
        let first = DraughtsMove {
            from: 77,
            to: 55,
            captured: Some(66),
        };
        assert!(game.moves().contains(&first));
        let next = game.apply_move(&first);

        assert_eq!(next.turn, -1, "the blocker must end the capture sequence");
        assert_eq!(next.forced, None);
        assert!(next.captured.is_empty());
        assert_eq!(next.board[66], 0, "captured men leave after the sequence");
        assert_eq!(
            next.board[88], -1,
            "the king may not cross the captured man"
        );
    }

    #[test]
    fn morris_placement_mill_removal_and_flying_are_legal() {
        let game = Morris::default();
        assert_eq!(game.moves().len(), 24);
        let game = game.apply_move(&MorrisMove::Place(0));
        assert_eq!(game.moves().len(), 23);
        let mut mill = Morris::default();
        mill.board[0] = 1;
        mill.board[1] = 1;
        mill.placed = [2, 0];
        let mill = mill.apply_move(&MorrisMove::Place(2));
        assert!(mill.removing);
        let mut flying = Morris::default();
        flying.board[0] = 1;
        flying.board[1] = 1;
        flying.board[2] = 1;
        flying.board[21] = -1;
        flying.board[22] = -1;
        flying.board[23] = -1;
        flying.placed = [9, 9];
        assert!(flying.moves().contains(&MorrisMove::Slide(0, 20)));
    }

    #[test]
    fn kalah_sowing_capture_extra_turn_and_endgame_are_pinned() {
        let game = Kalah::default();
        assert_eq!(game.moves(), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(game.apply_move(2).turn, 1);
        let mut capture = Kalah {
            pits: [0; 14],
            turn: 1,
        };
        capture.pits[0] = 1;
        capture.pits[7] = 1;
        capture.pits[11] = 3;
        capture.pits[5] = 1;
        let capture = capture.apply_move(0);
        assert_eq!(capture.pits[6], 4);
        let mut ending = Kalah {
            pits: [0; 14],
            turn: 1,
        };
        ending.pits[5] = 1;
        ending.pits[7] = 4;
        let ending = ending.apply_move(5);
        assert_eq!(ending.pits[13], 4);
        assert!(ending.pits[0..6].iter().all(|&n| n == 0));
        assert!(ending.pits[7..13].iter().all(|&n| n == 0));
    }

    #[test]
    fn a_denied_autosave_is_announced_until_a_write_lands() {
        use kobo_sdk::{AppRunner, StoreError};

        let mut app = Parlor::default();
        app.start(Title::Reversi);
        let mut runner = AppRunner::new(app);
        runner.store_result(StoreResult::Denied(StoreError::NoRoom));
        assert!(runner.app().save_failed);
        let words = shown(&runner.app().screen()).join(" | ");
        assert!(words.contains("Progress is not saved"), "{words}");
        runner.store_result(StoreResult::Saved {
            key: SAVE.to_owned(),
        });
        assert!(!runner.app().save_failed);
        let words = shown(&runner.app().screen()).join(" | ");
        assert!(!words.contains("Progress is not saved"), "{words}");
    }

    #[test]
    fn autosave_round_trips_every_position_kind() {
        for title in GAMES {
            let mut app = Parlor::default();
            app.start(title);
            let text = encode(&app, app.position.as_ref().unwrap());
            let restored = decode(&text).unwrap();
            assert_eq!(restored.position, app.position);
        }
    }

    #[test]
    fn autosave_restores_history_needed_for_confirmed_undo() {
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(Title::Reversi);
        let opening = app.position.clone();
        app.tap_reversi(Reversi::default(), 19);
        assert_eq!(app.history.len(), 1);

        let saved = encode(&app, app.position.as_ref().unwrap());
        let mut restored = decode(&saved).unwrap();
        assert_eq!(restored.history, app.history);
        restored.undo();
        assert!(restored.undo_pending);
        restored.undo();
        assert_eq!(restored.position, opening);
        assert!(restored.history.is_empty());
    }

    #[test]
    fn a_draughts_game_saved_while_ten_by_ten_was_chosen_in_setup_still_opens() {
        // Choosing the ten by ten rules and then backing out of setup wrote
        // a save that called an eight by eight game ten by ten, and the game
        // was refused the next time the reader opened it.
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(Title::Draughts);
        assert!(play_one_move(&mut app));
        let saved = encode(&app, app.position.as_ref().expect("a position"));
        let (header, rest) = saved.split_once(';').expect("a header");
        let mut fields: Vec<_> = header.split(',').collect();
        fields[1] = "1";
        let old = format!("{};{rest}", fields.join(","));
        let restored = decode(&old).expect("the game opens");
        assert_eq!(restored.position, app.position);
        assert_eq!(restored.history, app.history);
    }

    #[test]
    fn a_ten_by_ten_save_is_refused_rather_than_drawn_on_a_panel_too_small_for_it() {
        let game = Draughts::new(Rules::International);
        let app = Parlor {
            position: Some(Position::Draughts(game)),
            ..Parlor::default()
        };
        let saved = encode(&app, app.position.as_ref().expect("a position"));
        assert!(decode(&saved).is_none());
    }

    #[test]
    fn malformed_autosaves_are_refused_before_they_can_reach_the_board() {
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(Title::Reversi);
        let saved = encode(&app, app.position.as_ref().expect("position"));
        assert!(decode(&saved).is_some());

        let bad_turn = saved.replacen("0,0,1,1,1,1,0,0,0,-", "0,0,1,1,1,0,0,0,0,-", 1);
        assert!(decode(&bad_turn).is_none());
        assert!(decode("0,0,1,1,1,1,0,0,0,-;D,-,-;2;;").is_none());
        let bad_selected = saved.replacen("0,0,1,1,1,1,0,0,0,-", "0,0,1,1,1,1,0,0,0,99", 1);
        assert!(decode(&bad_selected).is_none());
    }

    #[test]
    fn deterministic_playouts_preserve_engine_invariants() {
        let mut reversi = Reversi::default();
        let mut occupied = 4;
        for _ in 0..80 {
            if reversi.terminal_score(1).is_some() {
                break;
            }
            let mv = reversi.legal_moves().remove(0);
            reversi = reversi.apply(&mv);
            let next_occupied = reversi.board.iter().filter(|&&piece| piece != 0).count();
            assert!(next_occupied >= occupied);
            assert!(reversi.board.iter().all(|piece| (-1..=1).contains(piece)));
            occupied = next_occupied;
        }
        assert!(reversi.terminal_score(1).is_some());

        for rules in [Rules::AngloAmerican, Rules::International] {
            let mut draughts = Draughts::new(rules);
            let mut pieces = draughts.board.iter().filter(|&&piece| piece != 0).count();
            for _ in 0..160 {
                if draughts.terminal_score(1).is_some() {
                    break;
                }
                let mv = draughts.legal_moves().remove(0);
                draughts = draughts.apply(&mv);
                let next_pieces = draughts.board.iter().filter(|&&piece| piece != 0).count();
                assert!(next_pieces <= pieces);
                assert!(draughts.board.iter().all(|piece| (-2..=2).contains(piece)));
                assert!(draughts.captured.iter().all(|&at| draughts.board[at] != 0));
                let mut unique = draughts.captured.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(unique.len(), draughts.captured.len());
                pieces = next_pieces;
            }
        }

        let mut morris = Morris::default();
        for _ in 0..160 {
            if morris.terminal_score(1).is_some() {
                break;
            }
            let mv = morris.legal_moves().remove(0);
            morris = morris.apply(&mv);
            assert!(morris.board.iter().all(|piece| (-1..=1).contains(piece)));
            assert!(morris.placed.iter().all(|&placed| placed <= 9));
            assert!(morris.count(1) <= usize::from(morris.placed[0]));
            assert!(morris.count(-1) <= usize::from(morris.placed[1]));
        }

        let mut kalah = Kalah::default();
        for _ in 0..160 {
            if kalah.terminal_score(1).is_some() {
                break;
            }
            let mv = kalah.legal_moves().remove(0);
            kalah = kalah.apply(&mv);
            assert_eq!(
                kalah
                    .pits
                    .iter()
                    .map(|&seeds| u16::from(seeds))
                    .sum::<u16>(),
                48
            );
        }
        assert!(kalah.terminal_score(1).is_some());
    }

    #[test]
    fn the_computer_plays_the_same_move_every_time_at_each_named_strength() {
        let game = Reversi::default();
        for strength in [Strength::Casual, Strength::Club, Strength::Strong] {
            assert_eq!(
                choose_move(&game, strength),
                choose_move(&game, strength),
                "{}",
                strength.name()
            );
        }
    }

    /// Taps whatever the rules say is legal next, the way a player would.
    ///
    /// One move may be two taps -- a piece then its destination -- and a
    /// capture chain is more, so this drives the application through its own
    /// action handling rather than reaching into the engine.
    fn play_one_move(app: &mut Parlor) -> bool {
        let Some(position) = app.position.clone() else {
            return false;
        };
        if position.terminal().is_some() {
            return false;
        }
        // The computer's own choice rather than the first legal move: a game
        // where both sides always take the first move on the list wanders,
        // and Nine Men's Morris in particular never ends. This is the same
        // engine the computer opponent plays with.
        let mut context = Context::default();
        match position {
            Position::Reversi(game) => {
                let Some(ReversiMove::Place(at)) = choose_move(&game, Strength::Casual) else {
                    return false;
                };
                app.on_action(&mut context, action_id(&cell_name(at)));
            }
            Position::Draughts(game) => {
                let Some(mv) = choose_move(&game, Strength::Casual) else {
                    return false;
                };
                app.on_action(&mut context, action_id(&cell_name(mv.from)));
                app.on_action(&mut context, action_id(&cell_name(mv.to)));
            }
            Position::Morris(game) => match choose_move(&game, Strength::Casual) {
                None => return false,
                Some(chosen) => match chosen {
                    MorrisMove::Place(to) | MorrisMove::Remove(to) => {
                        app.on_action(&mut context, action_id(&cell_name(MORRIS_GRID[to])));
                    }
                    MorrisMove::Slide(from, to) => {
                        app.on_action(&mut context, action_id(&cell_name(MORRIS_GRID[from])));
                        app.on_action(&mut context, action_id(&cell_name(MORRIS_GRID[to])));
                    }
                },
            },
            Position::Kalah(game) => {
                // A Kalah move is the pit it sows from, so the tap is the pit.
                let Some(pit) = choose_move(&game, Strength::Casual) else {
                    return false;
                };
                app.on_action(&mut context, action_id(&cell_name(pit)));
            }
        }
        true
    }

    #[test]
    fn every_game_can_be_played_to_its_end_and_picked_up_again() {
        // Through the application's own taps rather than the engine, because
        // the engine being right about the rules says nothing about whether
        // the panel lets anybody reach them.
        for (title, mode) in GAMES
            .into_iter()
            .flat_map(|title| [(title, Mode::Solo), (title, Mode::PassAndPlay)])
        {
            let mut app = Parlor {
                mode,
                ..Parlor::default()
            };
            app.start(title);
            let mut moves = 0;
            while play_one_move(&mut app) && moves < 400 {
                moves += 1;
            }
            let position = app.position.clone().expect("a position");
            assert!(
                position.terminal().is_some(),
                "{} did not finish in {moves} moves",
                title.name()
            );
            assert!(moves > 0, "{} took no moves at all", title.name());
            assert!(
                app.match_score[0] + app.match_score[1] <= 2,
                "{} counted more rounds than were played",
                title.name()
            );
            let ended = app.screen();
            let words = shown(&ended);
            assert!(
                words
                    .iter()
                    .any(|line| line.contains("win") || line.contains("Draw")),
                "{} never said how it ended: {words:?}",
                title.name()
            );

            // And the same game, put down and picked up: the autosave is what
            // a reader who closes the cover mid-game is relying on.
            let text = encode(&app, app.position.as_ref().expect("a position"));
            let reopened = decode(&text)
                .unwrap_or_else(|| panic!("{} save was refused: {text}", title.name()));
            assert_eq!(
                reopened.position.as_ref().map(Position::turn),
                Some(position.turn()),
                "{} resumed on the wrong side",
                title.name()
            );
            assert_eq!(
                reopened.position.as_ref().and_then(Position::terminal),
                position.terminal(),
                "{} resumed as an unfinished game",
                title.name()
            );
        }
    }

    #[test]
    fn a_morris_shuffle_ends_in_a_draw_rather_than_going_on_forever() {
        // Two players who will not break their own mills can slide the same
        // two pieces back and forth for as long as they like, and the match
        // card above the board says best of three: a round that cannot end
        // cannot be scored.
        let mut game = Morris {
            placed: [9, 9],
            quiet: QUIET_LIMIT - 1,
            ..Morris::default()
        };
        game.board[0] = 1;
        game.board[1] = 1;
        game.board[2] = 1;
        game.board[9] = -1;
        game.board[10] = -1;
        game.board[11] = -1;
        assert_eq!(game.terminal_score(1), None);
        let quiet = game.apply_move(&game.moves().remove(0));
        assert_eq!(quiet.terminal_score(1), Some(0), "the shuffle never ended");
    }

    #[test]
    fn a_capture_chain_survives_being_put_down_on_the_board_this_panel_plays() {
        // The eight by eight rules keep a forced square with nothing waiting
        // to be lifted, and the save refused exactly that: a reader who shut
        // the cover in the middle of a double capture lost the game. The ten
        // by ten case has its own test above; this is the one that can
        // actually be played here.
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(Title::Draughts);
        let mut chained = false;
        for _ in 0..200 {
            if !play_one_move(&mut app) {
                break;
            }
            let forced = match app.position.as_ref() {
                Some(Position::Draughts(game)) => game.forced,
                _ => None,
            };
            if forced.is_some() {
                chained = true;
                let text = encode(&app, app.position.as_ref().expect("a position"));
                let reopened = decode(&text)
                    .unwrap_or_else(|| panic!("a chain in progress was refused: {text}"));
                assert_eq!(reopened.position, app.position);
                break;
            }
        }
        assert!(chained, "no capture chain came up in two hundred moves");
    }

    /// The cells a screen's board draws in its chosen state.
    fn marked_on(screen: &Screen) -> Vec<usize> {
        let mut marked = Vec::new();
        for node in &screen.nodes {
            if let kobo_ui::Node::Grid { cells, .. } = node {
                for cell in cells.iter().filter(|cell| cell.selected) {
                    if let Some(at) = (0..100).find(|at| cell.action == action_id(&cell_name(*at)))
                    {
                        marked.push(at);
                    }
                }
            }
        }
        marked.sort_unstable();
        marked
    }

    #[test]
    fn the_computer_answers_every_move_without_being_asked_in_every_game() {
        // Backgammon made the reader tap "Computer move" every turn. Here the
        // reply is part of the tap that ended the player's move.
        for title in GAMES {
            let mut app = Parlor::default();
            assert_eq!(app.mode, Mode::Solo);
            app.start(title);
            for _ in 0..12 {
                if !play_one_move(&mut app) {
                    break;
                }
                let position = app.position.as_ref().expect("a position");
                assert!(
                    position.terminal().is_some()
                        || position.turn() == app.human_side
                        || position.mid_move(),
                    "{}: still the computer's turn after the player moved",
                    title.name()
                );
            }
            assert!(
                app.record.iter().any(|line| line.starts_with("Computer:")),
                "{}: the computer never moved: {:?}",
                title.name(),
                app.record
            );
        }
    }

    #[test]
    fn the_computer_moves_first_at_once_when_the_player_takes_the_second_side() {
        for title in GAMES {
            let mut app = Parlor {
                human_side: -1,
                ..Parlor::default()
            };
            app.start(title);
            let position = app.position.as_ref().expect("a position");
            assert_eq!(position.turn(), -1, "{}", title.name());
            assert!(!app.history.is_empty(), "{}", title.name());
            assert!(!marked_on(&app.screen()).is_empty(), "{}", title.name());
        }
    }

    #[test]
    fn the_computers_last_move_stays_marked_until_the_player_moves_again() {
        let mut app = Parlor::default();
        app.start(Title::Reversi);
        assert!(marked_on(&app.screen()).is_empty(), "nothing has moved yet");
        assert!(play_one_move(&mut app));
        let steps = app.last_turn();
        let Some(Step::Reversi(before, ReversiMove::Place(placed))) = steps.first() else {
            panic!("the computer's reply was not found: {steps:?}");
        };
        assert_eq!(before.turn, -1);
        let screen = app.screen();
        assert_eq!(marked_on(&screen), [*placed]);
        let words = shown(&screen).join(" ");
        assert!(
            words.contains("The computer placed the marked disc and turned one of your discs."),
            "{words}"
        );
        assert!(app
            .record
            .last()
            .expect("a reply")
            .starts_with("Computer: "));

        // A wrong tap is not a move, so the mark stays.
        let filled = (0..64)
            .find(
                |&at| matches!(&app.position, Some(Position::Reversi(game)) if game.board[at] != 0),
            )
            .expect("a disc");
        app.handle_action(action_id(&cell_name(filled)));
        assert_eq!(marked_on(&app.screen()), [*placed]);

        // The player's next move is answered, and the mark moves with it.
        assert!(play_one_move(&mut app));
        let Some(Step::Reversi(_, ReversiMove::Place(next))) = app.last_turn().first().cloned()
        else {
            panic!("no second reply");
        };
        assert_eq!(marked_on(&app.screen()), [next]);
    }

    #[test]
    fn each_game_marks_what_the_computer_moved_in_its_own_terms() {
        // Draughts: where the piece came from and where it went, and what it
        // jumped. Kalah: the pit it sowed. Morris draws no chosen state, so
        // the line under the status names the points in words.
        let mut draughts = Parlor::default();
        draughts.start(Title::Draughts);
        assert!(play_one_move(&mut draughts));
        let steps = draughts.last_turn();
        let Some(Step::Draughts(_, mv)) = steps.first() else {
            panic!("no draughts reply: {steps:?}");
        };
        let mut expected = vec![mv.from, mv.to];
        expected.extend(mv.captured);
        expected.sort_unstable();
        assert_eq!(marked_on(&draughts.screen()), expected);
        let words = shown(&draughts.screen()).join(" ");
        assert!(
            words.contains("The computer moved a piece along the marked squares."),
            "{words}"
        );

        let mut kalah = Parlor::default();
        kalah.start(Title::Kalah);
        // Pit 3 has four stones, and the fourth lands across the board, so
        // the turn passes to the computer.
        kalah.handle_action(action_id(&cell_name(3)));
        let marked = marked_on(&kalah.screen());
        assert!(!marked.is_empty());
        assert!(marked.iter().all(|pit| (7..13).contains(pit)), "{marked:?}");
        let words = shown(&kalah.screen()).join(" ");
        assert!(words.contains("The computer sowed"), "{words}");
        assert!(words.contains("You: "), "{words}");
        assert!(words.contains("Computer: "), "{words}");

        let mut morris = Parlor::default();
        morris.start(Title::Morris);
        morris.handle_action(action_id(&cell_name(MORRIS_GRID[0])));
        let words = shown(&morris.screen()).join(" ");
        assert!(
            words.contains("The computer placed a piece on the "),
            "{words}"
        );
        assert!(words.contains(" square's "), "{words}");
    }

    #[test]
    fn morris_points_are_named_for_what_can_be_seen_on_the_board() {
        assert_eq!(morris_point(0, false), "the outer square's top left corner");
        assert_eq!(
            morris_point(0, true),
            "the outer square's bottom right corner"
        );
        assert_eq!(
            morris_point(4, false),
            "the middle of the middle square's top side"
        );
        assert_eq!(
            morris_point(11, false),
            "the middle of the inner square's left side"
        );
        assert_eq!(
            morris_point(23, false),
            "the outer square's bottom right corner"
        );
        let mut names: Vec<_> = (0..24).map(|point| morris_point(point, false)).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 24, "two points share a name");
    }

    #[test]
    fn between_two_players_the_other_players_move_is_marked_too() {
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(Title::Reversi);
        app.handle_action(action_id(&cell_name(19)));
        assert_eq!(marked_on(&app.screen()), [19]);
        let words = shown(&app.screen()).join(" ");
        assert!(
            words.contains("Black placed the marked disc and turned one white disc."),
            "{words}"
        );
        assert!(
            words.contains("Board turned. Pass the reader to White."),
            "{words}"
        );
        assert!(words.contains("White to move"), "{words}");
    }

    #[test]
    fn a_new_install_plays_the_computer_and_pass_and_play_is_one_tap_away() {
        let app = Parlor::default();
        assert_eq!(app.mode, Mode::Solo);
        let mut setup = Parlor {
            view: View::Setup(Title::Reversi),
            ..Parlor::default()
        };
        let words = shown(&setup.screen()).join(" | ");
        assert!(words.contains("Computer strength"), "{words}");
        setup.handle_action(action_id(PLAY_PEOPLE));
        assert_eq!(setup.mode, Mode::PassAndPlay);
        setup.handle_action(action_id("start"));
        let words = shown(&setup.screen()).join(" | ");
        assert!(words.contains("Black to move"), "{words}");

        // A game saved between two people opens as one.
        let saved = encode(&setup, setup.position.as_ref().expect("a position"));
        assert_eq!(decode(&saved).expect("opens").mode, Mode::PassAndPlay);
    }

    #[test]
    fn no_screen_speaks_of_the_house_or_of_compass_points() {
        let mut screens = Vec::new();
        for title in GAMES {
            for mode in [Mode::Solo, Mode::PassAndPlay] {
                let mut app = Parlor {
                    mode,
                    ..Parlor::default()
                };
                app.view = View::Setup(title);
                screens.push(app.screen());
                app.human_side = -1;
                screens.push(app.screen());
                app.start(title);
                for _ in 0..6 {
                    play_one_move(&mut app);
                    screens.push(app.screen());
                }
                app.view = View::Record;
                screens.push(app.screen());
                app.view = View::Rules(title, false);
                screens.push(app.screen());
            }
        }
        screens.push(Parlor::default().screen());
        for screen in &screens {
            let words = shown(screen).join(" ");
            for jargon in ["house", "House", "South", "North", "International", "Solo"] {
                assert!(!words.contains(jargon), "{jargon}: {words}");
            }
        }
    }

    #[test]
    fn the_play_screen_keeps_undo_rules_and_new_game_where_every_game_keeps_them() {
        let mut app = Parlor::default();
        app.start(Title::Reversi);
        let screen = app.screen();
        let bar = screen.top_bar.as_ref().expect("a top bar");
        let labels: Vec<_> = bar
            .actions
            .iter()
            .map(|action| action.label.as_str())
            .collect();
        assert!(labels.contains(&chrome::RULES_LABEL), "{labels:?}");
        let foot = screen.nav_bar.as_ref().expect("a bar at the foot");
        let foot: Vec<_> = foot
            .destinations
            .iter()
            .map(|action| action.label.as_str())
            .collect();
        assert_eq!(foot, [chrome::UNDO_LABEL, "Moves"]);
        let words = shown(&screen).join(" | ");
        assert!(!words.contains("How to play"), "{words}");

        app.handle_action(action_id(chrome::MORE));
        let open = app.screen();
        let entries: Vec<_> = open
            .overlay
            .as_ref()
            .expect("the menu")
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_ui::Node::Button { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(entries, [chrome::NEW_GAME_LABEL, "Change game"]);

        // A tap beside the menu closes it and leaves the game where it was.
        app.handle_action(ActionId::BACK);
        assert_eq!(app.view, View::Board);
        assert!(!app.menu.is_open());

        // Rules is one tap, and Back comes straight back to the game.
        app.handle_action(action_id(chrome::RULES));
        assert_eq!(app.view, View::Rules(Title::Reversi, false));
        app.handle_action(ActionId::BACK);
        assert_eq!(app.view, View::Board);

        // New game goes through setup, which says what it would replace, and
        // Back from there is the game again rather than the shelf.
        play_one_move(&mut app);
        app.handle_action(action_id(chrome::MORE));
        app.handle_action(action_id(chrome::NEW_GAME));
        assert_eq!(app.view, View::Setup(Title::Reversi));
        let words = shown(&app.screen()).join(" | ");
        assert!(
            words.contains("Starting replaces the game in progress."),
            "{words}"
        );
        app.handle_action(ActionId::BACK);
        assert_eq!(app.view, View::Board);

        app.handle_action(action_id(chrome::MORE));
        app.handle_action(action_id(CHANGE_GAME));
        assert_eq!(app.view, View::Menu);
    }

    #[test]
    fn undo_against_the_computer_takes_back_the_move_and_the_reply_together() {
        for title in GAMES {
            let mut app = Parlor::default();
            app.start(title);
            assert!(play_one_move(&mut app));
            let before_second = (app.position.clone(), app.history.len(), app.record.clone());
            assert!(play_one_move(&mut app));
            app.handle_action(action_id(chrome::UNDO));
            assert_eq!(
                (app.position.clone(), app.history.len(), app.record.clone()),
                before_second,
                "{}",
                title.name()
            );
            // Back where the first reply had just been played, so that reply
            // is marked again. A first Kalah sowing that ends in the store
            // has no reply yet.
            let replied = before_second
                .2
                .iter()
                .any(|line| line.starts_with("Computer:"));
            assert_eq!(!app.last_turn().is_empty(), replied, "{}", title.name());
            app.handle_action(action_id(chrome::UNDO));
            assert!(app.history.is_empty(), "{}", title.name());
            app.handle_action(action_id(chrome::UNDO));
            assert_eq!(app.notice, chrome::NOTHING_TO_UNDO);
        }
    }

    #[test]
    fn undo_against_the_computer_cannot_take_back_its_opening_move_alone() {
        // Going back to the computer's turn would only have it play the same
        // move again, so there is nothing to take back yet.
        let mut app = Parlor {
            human_side: -1,
            ..Parlor::default()
        };
        app.start(Title::Reversi);
        let opened = app.position.clone();
        app.handle_action(action_id(chrome::UNDO));
        assert_eq!(app.position, opened);
        assert_eq!(app.notice, chrome::NOTHING_TO_UNDO);
    }

    #[test]
    fn undo_between_two_players_waits_for_the_other_one_to_agree() {
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(Title::Reversi);
        app.handle_action(action_id(chrome::UNDO));
        assert_eq!(app.notice, chrome::NOTHING_TO_UNDO);
        assert!(!app.undo_pending);

        app.handle_action(action_id(&cell_name(19)));
        let moved = app.position.clone();
        app.handle_action(action_id(chrome::UNDO));
        assert!(app.undo_pending);
        assert_eq!(app.position, moved, "asking is not taking back");
        let words = shown(&app.screen()).join(" ");
        assert!(
            words.contains("The other player taps Undo to agree"),
            "{words}"
        );
        app.handle_action(action_id(chrome::UNDO));
        assert_eq!(app.position, Some(Position::Reversi(Reversi::default())));
        assert!(app.history.is_empty());
    }

    #[test]
    fn undo_takes_back_a_whole_capture_chain() {
        let mut game = Draughts::new(Rules::AngloAmerican);
        game.board.fill(0);
        game.board[42] = 1;
        game.board[33] = -1;
        game.board[17] = -1;
        game.board[7] = -1;
        let start = game.clone();
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            position: Some(Position::Draughts(game)),
            view: View::Board,
            ..Parlor::default()
        };
        app.handle_action(action_id(&cell_name(42)));
        app.handle_action(action_id(&cell_name(24)));
        app.handle_action(action_id(&cell_name(10)));
        assert!(matches!(&app.position, Some(Position::Draughts(g)) if g.turn == -1));
        assert_eq!(app.history.len(), 2);
        app.handle_action(action_id(chrome::UNDO));
        app.handle_action(action_id(chrome::UNDO));
        assert_eq!(app.position, Some(Position::Draughts(start)));
    }

    #[test]
    fn a_legal_square_is_a_drawn_mark_rather_than_whatever_the_typeface_has() {
        // It used to be the dotted circle character, so what a legal square
        // looked like depended on the face the panel happened to be using.
        for title in [Title::Reversi, Title::Draughts] {
            let mut app = Parlor::default();
            app.start(title);
            if title == Title::Draughts {
                // A piece has to be in hand before its destinations show.
                let mv = match app.position.clone() {
                    Some(Position::Draughts(game)) => game.moves().remove(0),
                    _ => unreachable!("draughts"),
                };
                app.on_action(&mut Context::default(), action_id(&cell_name(mv.from)));
            }
            let drawn = format!("{:?}", app.screen());
            assert!(
                drawn.contains("LegalPoint"),
                "{}: no legal square was drawn: {drawn}",
                title.name()
            );
            assert!(
                !drawn.contains('\u{25cc}'),
                "{}: still a character",
                title.name()
            );
        }
    }

    #[test]
    fn draughts_offers_the_board_this_panel_can_draw_and_no_other() {
        // The ten by ten rules were a row on the setup screen that could
        // never be started, because the panel draws at most eighty-one cells.
        let setup = Parlor {
            view: View::Setup(Title::Draughts),
            ..Parlor::default()
        };
        let words = shown(&setup.screen()).join(" | ");
        assert!(!words.contains("10×10"), "{words}");
        assert!(words.contains("Start game"), "{words}");
        let mut app = Parlor::default();
        app.start(Title::Draughts);
        assert!(matches!(
            &app.position,
            Some(Position::Draughts(game)) if game.rules == Rules::AngloAmerican
        ));
    }

    /// Every line of words a screen draws.
    fn shown(screen: &Screen) -> Vec<String> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    #[test]
    fn every_screen_fits_the_panel_at_every_text_size() {
        // A board shrinks to fit, and everything around it does not: the
        // score card, the line saying what just happened, the three controls
        // under the board. This is the sweep that says so at the sizes a
        // reader can actually choose, under the status band the reader draws
        // above every screen. Upright only: held sideways, a square board and
        // the lines around it do not fit a Clara at any text size, and that
        // needs a layout of its own with the board beside the controls.
        let chrome = Chrome::measuring(true);
        let mut failures = Vec::new();
        for scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            // Every screen is built through a runner at the size it is checked
            // at, the way the runtime builds it. Built first, the runner also
            // installs the real face before anything is measured, so the
            // smallest size is not measured in the fallback type when this test
            // runs on its own.
            let context = kobo_sdk::AppRunner::with_metrics(Parlor::default(), metrics).context();
            // The shelf itself, which is the screen every reader sees first,
            // in both of its states: bare, and carrying the offer to pick up a
            // game that is already under way.
            let mut resumable = Parlor::default();
            resumable.start(Title::Reversi);
            play_one_move(&mut resumable);
            resumable.view = View::Menu;
            for (name, table) in [("table", Parlor::default()), ("table resuming", resumable)] {
                let issues = table
                    .screen_for(&context)
                    .diagnostics(&metrics, &chrome)
                    .issues
                    .into_iter()
                    .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                    .collect::<Vec<_>>();
                if !issues.is_empty() {
                    failures.push(format!("{scale:?} {name}: {issues:?}"));
                }
            }
            for (title, mode) in GAMES
                .into_iter()
                .flat_map(|title| [(title, Mode::Solo), (title, Mode::PassAndPlay)])
            {
                let mut app = Parlor {
                    mode,
                    ..Parlor::default()
                };
                app.start(title);
                // Several moves in, so the line naming the other side's last
                // turn is as long as it gets: a Morris placement with a
                // removal names two points.
                for _ in 0..8 {
                    play_one_move(&mut app);
                }
                let board = app.screen_for(&context);
                // One move more hands the reader to the second player, whose
                // board is drawn turned round. Whose move it is must be read
                // whole beside the score, never cut short.
                play_one_move(&mut app);
                let handed_over = app.screen_for(&context);
                if let Some(position) = &app.position {
                    let status = app.status(position);
                    let layout = handed_over.layout_with(&metrics, &chrome);
                    let said = layout
                        .nodes
                        .iter()
                        .find(|node| matches!(node.kind, kobo_ui::LayoutKind::Section))
                        .and_then(|node| node.text_lines.first().cloned());
                    if said.as_deref() != Some(status.as_str()) {
                        failures.push(format!(
                            "{scale:?} {} {mode:?}: status {status:?} drawn as {said:?}",
                            title.name()
                        ));
                    }
                }
                app.handle_action(action_id(chrome::MORE));
                let menu = app.screen_for(&context);
                app.handle_action(ActionId::BACK);
                // The record is paged for the size it is read at, the way the
                // runtime pages it, so it is built against that size.
                app.view = View::Record;
                let record = app.screen_for(&context);
                app.view = View::Rules(title, false);
                let rules = app.screen_for(&context);
                // Setup over a game in progress carries the line saying what
                // starting would replace.
                app.view = View::Setup(title);
                let setup = app.screen_for(&context);
                app.view = View::Board;
                app.save_failed = true;
                let save_failed = app.screen_for(&context);
                for (name, screen) in [
                    ("setup", setup),
                    ("board", board),
                    ("board handed over", handed_over),
                    ("menu", menu),
                    ("record", record),
                    ("rules", rules),
                    ("save failed", save_failed),
                ] {
                    let issues = screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .into_iter()
                        .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                        .map(|issue| format!("{:?}", issue.kind))
                        .collect::<Vec<_>>();
                    if !issues.is_empty() {
                        failures.push(format!(
                            "{scale:?} {} {mode:?} {name}: {issues:?}",
                            title.name()
                        ));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn all_primary_screens_fit_clara_bw() {
        let chrome = Chrome::default();
        let app = Parlor::default();
        let menu_issues = app.screen().diagnostics(&CLARA_BW_METRICS, &chrome).issues;
        assert!(menu_issues.is_empty(), "menu: {menu_issues:?}");
        for title in GAMES {
            let mut app = Parlor {
                view: View::Setup(title),
                ..Parlor::default()
            };
            assert!(
                app.screen()
                    .diagnostics(&CLARA_BW_METRICS, &chrome)
                    .issues
                    .is_empty(),
                "setup: {}",
                title.name()
            );
            app.start(title);
            let screen = app.screen();
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &chrome);
            assert!(
                diagnostics.issues.is_empty(),
                "{}: {:?}",
                title.name(),
                diagnostics.issues
            );
            assert!(screen
                .layout_with(&CLARA_BW_METRICS, &chrome)
                .rect_of_action(action_id(chrome::UNDO))
                .is_some());
        }
    }
}

#[cfg(test)]
mod navigation_tests;

#[cfg(test)]
mod large_text_tests {
    use super::*;

    fn panels() -> impl Iterator<Item = kobo_sdk::DisplayMetrics> {
        [(1072, 1448, 300), (1264, 1680, 300), (1404, 1872, 227)]
            .into_iter()
            .flat_map(|(width, height, pixels_per_inch)| {
                [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest]
                    .into_iter()
                    .map(move |text_scale| kobo_sdk::DisplayMetrics {
                        width,
                        height,
                        pixels_per_inch,
                        text_scale,
                    })
            })
    }
    fn fits(screen: &Screen, metrics: kobo_sdk::DisplayMetrics) {
        let diagnostics = screen.diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
        assert!(
            !diagnostics.has_errors(),
            "{metrics:?}: {:#?}",
            diagnostics.issues
        );
    }

    #[test]
    fn save_failure_after_a_move_keeps_board_actions_reachable() {
        for metrics in panels() {
            let mut app = Parlor::default();
            app.start(Title::Reversi);
            app.handle_action(action_id("cell-19"));
            app.save_failed = true;
            let runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
            let screen = runner.app().screen_for(&runner.context());
            fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &kobo_sdk::Chrome::measuring(true));
            for name in [chrome::UNDO, "record", chrome::MORE, chrome::RULES] {
                let action = action_id(name);
                let rect = layout.rect_of_action(action).expect("board action");
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action)
                );
            }
        }
    }
}
