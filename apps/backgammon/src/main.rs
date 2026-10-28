//! Complete, touch-first backgammon rules for a portrait Kobo panel.
use kobo_sdk::entropy::{Entropy, FixtureEntropy, SystemEntropy};
use kobo_sdk::game::{self, Menu, Tap};
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, PictureHandle, Screen, ScreenBuilder,
    StoreResult, TilePicture,
};
use std::process::ExitCode;

const POINTS: usize = 24;
const CHECKERS: u8 = 15;
const MAX_CUBE: u8 = 64;
const SAVE: &str = "backgammon-autosave-v4";
/// Shown in place of the usual note until a write lands again.
const SAVE_FAILED: &str =
    "Progress is not saved. Keep the app open while you make room on your reader; the next move tries again.";
/// The save this replaces. A game left half-played survives the upgrade.
const OLD_SAVE: &str = "backgammon-autosave-v3";
/// How many finished turns the board remembers out loud.
const HISTORY: usize = 8;
/// Set to a number to play a fixed sequence of rolls, for captures and
/// fixtures. Named on screen whenever it is in use, because a recorded game
/// that looks like chance and is not would be a lie about the dice.
const SEED: &str = "KOBO_BACKGAMMON_SEED";
/// The numbers along the top edge of the board, and along the bottom.
///
/// The points themselves are the runtime's backgammon cells, because a cell
/// is something a finger can land on and a picture is not: the board used to
/// be one drawing with a row of numbered keys under it, so a move meant
/// finding the point on the drawing and then finding its number again among
/// the keys. The runtime draws the points and the checkers but not their
/// numbers, so each row is numbered by a strip drawn here, at the panel's own
/// width and one pixel to one pixel, which puts every number beside the point
/// it names.
const TOP_BAND: PictureHandle = PictureHandle(1);
const BOTTOM_BAND: PictureHandle = PictureHandle(2);
/// How tall each strip of point numbers is, in panel pixels.
const BAND_HEIGHT: u32 = 40;
/// The tallest stack the runtime draws on one point. Anything above it is
/// written as a count in the strip beside the point.
const DRAWN_STACK: u8 = 5;
/// What the menu offers after New game.
const MORE_ITEMS: [(&str, &str); 2] = [
    ("match-settings", "Match settings"),
    ("turn-history", "Turn history"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Player {
    White,
    Black,
}

impl Player {
    const fn other(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }
    const fn index(self) -> usize {
        match self {
            Self::White => 0,
            Self::Black => 1,
        }
    }
    const fn sign(self) -> i8 {
        match self {
            Self::White => 1,
            Self::Black => -1,
        }
    }
    const fn step(self) -> i8 {
        -self.sign()
    }
    const fn name(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Black => "Black",
        }
    }
}

/// Where the dice come from.
///
/// Backgammon is the dice. Before this the two numbers came off a counter that
/// advanced by one each roll, so every game dealt the same sequence in the same
/// order: that is not a game of chance, it is a recital. The operating system
/// is the source now, a seed can be named for a fixture, and a source that
/// cannot be opened says so on the board instead of falling back to anything.
enum Dice {
    System(SystemEntropy),
    Fixture(FixtureEntropy),
    Unavailable(String),
    /// An exact sequence, for tests that are about the rules rather than the
    /// dice: "roll 3 and 1, then 5 and 5" reads as the game it describes.
    #[cfg(test)]
    Scripted(std::collections::VecDeque<(u8, u8)>),
}

impl Dice {
    fn open() -> Self {
        match std::env::var(SEED) {
            Ok(value) => match value.trim().parse::<u64>() {
                Ok(seed) => Self::Fixture(FixtureEntropy::new(seed)),
                Err(_) => Self::Unavailable(format!("{SEED} is not a number.")),
            },
            Err(_) => match SystemEntropy::open() {
                Ok(source) => Self::System(source),
                Err(error) => Self::Unavailable(format!("No dice: {error}.")),
            },
        }
    }

    #[cfg(test)]
    fn scripted(rolls: impl IntoIterator<Item = (u8, u8)>) -> Self {
        Self::Scripted(rolls.into_iter().collect())
    }

    /// Two dice, or why there are none.
    fn roll(&mut self) -> Result<(u8, u8), String> {
        let source: &mut dyn Entropy = match self {
            Self::System(source) => source,
            Self::Fixture(source) => source,
            Self::Unavailable(problem) => return Err(problem.clone()),
            #[cfg(test)]
            Self::Scripted(rolls) => {
                return rolls
                    .pop_front()
                    .ok_or_else(|| "the script ran out of rolls".to_owned())
            }
        };
        let mut die = || {
            source
                .below(6)
                .map(|value| u8::try_from(value + 1).unwrap_or(1))
                .map_err(|error| format!("The dice could not be rolled: {error}."))
        };
        Ok((die()?, die()?))
    }

    /// What to say about these dice, when there is anything to say.
    fn provenance(&self) -> Option<String> {
        match self {
            Self::System(_) => None,
            #[cfg(test)]
            Self::Scripted(_) => Some("Scripted dice.".to_owned()),
            Self::Fixture(source) => Some(format!("Fixture dice, seed {}.", source.seed())),
            Self::Unavailable(problem) => Some(problem.clone()),
        }
    }
}

impl std::fmt::Debug for Dice {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::System(_) => "Dice::System",
            Self::Fixture(_) => "Dice::Fixture",
            Self::Unavailable(_) => "Dice::Unavailable",
            #[cfg(test)]
            Self::Scripted(_) => "Dice::Scripted",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Solo,
    PassAndPlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitialLoad {
    Pending,
    Settled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Move {
    from: Option<usize>,
    to: Option<usize>,
    die: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Selected {
    Bar,
    Point(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Position {
    points: [i8; POINTS],
    bar: [u8; 2],
    off: [u8; 2],
}

impl Position {
    fn initial() -> Self {
        let mut points = [0; POINTS];
        points[23] = 2;
        points[12] = 5;
        points[7] = 3;
        points[5] = 5;
        points[0] = -2;
        points[11] = -5;
        points[16] = -3;
        points[18] = -5;
        Self {
            points,
            bar: [0; 2],
            off: [0; 2],
        }
    }

    fn all_home(&self, player: Player) -> bool {
        let home = match player {
            Player::White => 0..=5,
            Player::Black => 18..=23,
        };
        self.bar[player.index()] == 0
            && self
                .points
                .iter()
                .enumerate()
                .all(|(point, checkers)| *checkers * player.sign() <= 0 || home.contains(&point))
    }

    fn has_checker_beyond(&self, player: Player, point: usize) -> bool {
        match player {
            Player::White => ((point + 1)..POINTS).any(|index| self.points[index] > 0),
            Player::Black => (0..point).any(|index| self.points[index] < 0),
        }
    }

    fn can_land(&self, player: Player, point: usize) -> bool {
        self.points[point] * player.sign() >= -1
    }

    fn single_moves(&self, player: Player, die: u8) -> Vec<Move> {
        if !(1..=6).contains(&die) {
            return Vec::new();
        }
        if self.bar[player.index()] > 0 {
            let entry = match player {
                Player::White => POINTS - usize::from(die),
                Player::Black => usize::from(die - 1),
            };
            return self
                .can_land(player, entry)
                .then_some(Move {
                    from: None,
                    to: Some(entry),
                    die,
                })
                .into_iter()
                .collect();
        }
        let mut moves = Vec::new();
        for from in 0..POINTS {
            if self.points[from].signum() != player.sign() {
                continue;
            }
            let target = i16::try_from(from).expect("point index fits i16")
                + i16::from(player.step()) * i16::from(die);
            if (0..i16::try_from(POINTS).expect("point count fits i16")).contains(&target) {
                let to = usize::try_from(target).expect("checked non-negative point");
                if self.can_land(player, to) {
                    moves.push(Move {
                        from: Some(from),
                        to: Some(to),
                        die,
                    });
                }
                continue;
            }
            let exact = match player {
                Player::White => die == u8::try_from(from + 1).expect("point distance fits"),
                Player::Black => die == u8::try_from(POINTS - from).expect("point distance fits"),
            };
            if self.all_home(player)
                && (target < 0 || target >= i16::try_from(POINTS).expect("point count fits i16"))
                && (exact || !self.has_checker_beyond(player, from))
            {
                moves.push(Move {
                    from: Some(from),
                    to: None,
                    die,
                });
            }
        }
        moves
    }

    fn apply(&mut self, player: Player, play: Move) {
        if let Some(from) = play.from {
            self.points[from] -= player.sign();
        } else {
            self.bar[player.index()] -= 1;
        }
        if let Some(to) = play.to {
            if self.points[to] == -player.sign() {
                self.points[to] = 0;
                self.bar[player.other().index()] += 1;
            }
            self.points[to] += player.sign();
        } else {
            self.off[player.index()] += 1;
        }
    }

    fn won(&self, player: Player) -> bool {
        self.off[player.index()] == CHECKERS
    }

    fn points_worth(&self, winner: Player) -> u8 {
        let loser = winner.other();
        if self.off[loser.index()] != 0 {
            return 1;
        }
        let loser_in_winner_home = match winner {
            Player::White => (0..=5).any(|point| self.points[point] < 0),
            Player::Black => (18..=23).any(|point| self.points[point] > 0),
        };
        if self.bar[loser.index()] > 0 || loser_in_winner_home {
            3
        } else {
            2
        }
    }
}

fn legal_turns(position: &Position, player: Player, dice: &[u8]) -> Vec<Vec<Move>> {
    fn visit(position: &Position, player: Player, dice: &[u8]) -> Vec<Vec<Move>> {
        if dice.is_empty() {
            return vec![Vec::new()];
        }
        let mut out = Vec::new();
        for index in 0..dice.len() {
            if dice[..index].contains(&dice[index]) {
                continue;
            }
            for play in position.single_moves(player, dice[index]) {
                let mut next = position.clone();
                next.apply(player, play);
                let mut remaining = dice.to_vec();
                remaining.remove(index);
                for tail in visit(&next, player, &remaining) {
                    let mut sequence = Vec::with_capacity(tail.len() + 1);
                    sequence.push(play);
                    sequence.extend(tail);
                    if !out.contains(&sequence) {
                        out.push(sequence);
                    }
                }
            }
        }
        if out.is_empty() {
            vec![Vec::new()]
        } else {
            out
        }
    }

    let all = visit(position, player, dice);
    let maximum = all.iter().map(Vec::len).max().unwrap_or(0);
    let mut legal: Vec<_> = all
        .into_iter()
        .filter(|sequence| sequence.len() == maximum && !sequence.is_empty())
        .collect();
    if maximum == 1 && dice.len() == 2 && dice[0] != dice[1] {
        let higher = dice[0].max(dice[1]);
        if legal.iter().any(|sequence| sequence[0].die == higher) {
            legal.retain(|sequence| sequence[0].die == higher);
        }
    }
    legal
}

#[derive(Clone)]
struct Snapshot {
    position: Position,
    turn: Player,
    dice: Vec<u8>,
    cube: u8,
    cube_owner: Option<Player>,
    score: [u8; 2],
    crawford_active: bool,
    opening: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Playing,
    ConfirmDouble,
    Offered(Player),
    ConfirmDrop(Player),
    GameOver(Player),
    MatchOver(Player),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Board,
    /// Everything about the match rather than the move: who is playing, how
    /// long it runs, and where the dice came from. Off the board, because a
    /// control that changes the match has no business beside the controls
    /// that play it.
    Match,
    /// The rules, one tap from the board.
    Help,
    /// Asking before New game throws away a match in progress.
    NewMatch,
    History,
}

/// What the side that moved last did, kept so the board can point at it.
///
/// The computer's whole turn lands in one repaint, and a board that has
/// simply changed is a puzzle: which of these checkers moved? Its points are
/// marked until the next roll. Not saved, because after a restart the turn it
/// describes is no longer the last thing the reader saw.
#[derive(Clone, Debug, Eq, PartialEq)]
struct LastTurn {
    player: Player,
    plays: Vec<Move>,
}

impl Phase {
    fn encode(self) -> String {
        match self {
            Self::Playing => "playing".into(),
            Self::ConfirmDouble => "confirm-double".into(),
            Self::Offered(Player::White) => "offered-white".into(),
            Self::Offered(Player::Black) => "offered-black".into(),
            Self::ConfirmDrop(Player::White) => "drop-white".into(),
            Self::ConfirmDrop(Player::Black) => "drop-black".into(),
            Self::GameOver(Player::White) => "game-over-white".into(),
            Self::GameOver(Player::Black) => "game-over-black".into(),
            Self::MatchOver(Player::White) => "match-over-white".into(),
            Self::MatchOver(Player::Black) => "match-over-black".into(),
        }
    }

    fn decode(text: &str) -> Option<Self> {
        match text {
            "playing" => Some(Self::Playing),
            "confirm-double" => Some(Self::ConfirmDouble),
            "offered-white" => Some(Self::Offered(Player::White)),
            "offered-black" => Some(Self::Offered(Player::Black)),
            "drop-white" => Some(Self::ConfirmDrop(Player::White)),
            "drop-black" => Some(Self::ConfirmDrop(Player::Black)),
            "game-over-white" => Some(Self::GameOver(Player::White)),
            "game-over-black" => Some(Self::GameOver(Player::Black)),
            "match-over-white" => Some(Self::MatchOver(Player::White)),
            "match-over-black" => Some(Self::MatchOver(Player::Black)),
            _ => None,
        }
    }
}

/// Whether the last autosave landed, so a refusal stays on screen until a
/// write does.
#[derive(Clone, Copy, Eq, PartialEq)]
enum SaveState {
    Kept,
    Refused,
}

struct Game {
    position: Position,
    turn: Player,
    dice: Vec<u8>,
    selected: Option<Selected>,
    history_page: usize,
    rules_page: usize,
    cube: u8,
    cube_owner: Option<Player>,
    score: [u8; 2],
    match_to: u8,
    crawford_used: bool,
    crawford_active: bool,
    opening: bool,
    mode: Mode,
    /// Where the dice come from. Not saved: a source is opened per session,
    /// and a saved seed would outlive the run that asked for one.
    dice_source: Dice,
    /// What each side has played, most recent last, in board notation.
    played: Vec<String>,
    /// The moves of the turn in progress, which become one line of that record.
    turn_plays: Vec<Move>,
    last_turn: Option<LastTurn>,
    phase: Phase,
    history: Vec<Snapshot>,
    message: String,
    save: SaveState,
    initial_load: InitialLoad,
    view: View,
    menu: Menu,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            position: Position::initial(),
            turn: Player::White,
            dice: Vec::new(),
            selected: None,
            history_page: 0,
            rules_page: 0,
            cube: 1,
            cube_owner: None,
            score: [0; 2],
            match_to: 5,
            crawford_used: false,
            crawford_active: false,
            opening: true,
            mode: Mode::Solo,
            dice_source: Dice::open(),
            played: Vec::new(),
            turn_plays: Vec::new(),
            last_turn: None,
            phase: Phase::Playing,
            history: Vec::new(),
            message: opening_message(Mode::Solo),
            save: SaveState::Kept,
            initial_load: InitialLoad::Pending,
            view: View::Board,
            menu: Menu::default(),
        }
    }
}

impl Game {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            position: self.position.clone(),
            turn: self.turn,
            dice: self.dice.clone(),
            cube: self.cube,
            cube_owner: self.cube_owner,
            score: self.score,
            crawford_active: self.crawford_active,
            opening: self.opening,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.position = snapshot.position;
        self.turn = snapshot.turn;
        self.dice = snapshot.dice;
        self.cube = snapshot.cube;
        self.cube_owner = snapshot.cube_owner;
        self.score = snapshot.score;
        self.crawford_active = snapshot.crawford_active;
        self.opening = snapshot.opening;
        self.selected = None;
        self.phase = Phase::Playing;
        self.message = "Move taken back.".into();
    }

    /// Takes back the last move of the turn in progress, or says there is
    /// none. A finished turn is final, as it is across a real table.
    fn undo(&mut self) {
        let human = !(self.mode == Mode::Solo && self.turn == Player::Black);
        if self.phase == Phase::Playing && human {
            if let Some(snapshot) = self.history.pop() {
                self.restore(snapshot);
                self.turn_plays.pop();
                return;
            }
        }
        self.message = game::NOTHING_TO_UNDO.into();
    }

    /// What the board calls a side: "You" and "Computer" when one of them is
    /// the computer, their colours when two people share the reader.
    const fn side(&self, player: Player) -> &'static str {
        match (self.mode, player) {
            (Mode::Solo, Player::White) => "You",
            (Mode::Solo, Player::Black) => "Computer",
            (Mode::PassAndPlay, _) => player.name(),
        }
    }

    /// Whose numbers are printed beside the points.
    ///
    /// Against the computer the board never turns, so it always reads from
    /// White's side, and the computer's moves are written in those numbers
    /// too: "Computer played 1/5" points at the 1 and the 5 the player can
    /// see, where Black's own count of 24/20 would point at two other places
    /// on the same board. When two people share the reader the board turns to
    /// whoever is moving, and so do the numbers.
    const fn viewer(&self) -> Player {
        match self.mode {
            Mode::Solo => Player::White,
            Mode::PassAndPlay => self.turn,
        }
    }

    /// Who rolls next, as a sentence.
    fn to_roll(&self) -> String {
        match (self.mode, self.turn) {
            (Mode::Solo, Player::White) => "Your roll.".to_owned(),
            (Mode::Solo, Player::Black) => "Computer to roll.".to_owned(),
            (Mode::PassAndPlay, player) => format!("{} to roll.", player.name()),
        }
    }

    /// Whether the computer is the side to play, and so plays now.
    fn computer_due(&self) -> bool {
        self.mode == Mode::Solo
            && self.turn == Player::Black
            && self.phase == Phase::Playing
            && !self.opening
    }

    fn take_roll(&mut self) -> Option<(u8, u8)> {
        match self.dice_source.roll() {
            Ok(roll) => Some(roll),
            Err(problem) => {
                self.message = problem;
                None
            }
        }
    }

    fn roll(&mut self) {
        if self.phase != Phase::Playing || !self.dice.is_empty() {
            return;
        }
        self.history.clear();
        let opening = self.opening;
        // An opening roll of a pair decides nothing, so it is thrown again.
        let mut roll = self.take_roll();
        while opening && roll.is_some_and(|(first, second)| first == second) {
            roll = self.take_roll();
        }
        let Some((first, second)) = roll else {
            return;
        };
        if opening {
            self.turn = if first > second {
                Player::White
            } else {
                Player::Black
            };
            self.opening = false;
        }
        // The marks for the turn before belong to the board as it was before
        // this roll; from here on the board marks what can move now.
        self.last_turn = None;
        self.dice = if first == second {
            vec![first; 4]
        } else {
            vec![first, second]
        };
        let side = self.side(self.turn);
        let rolled = if opening { "opened with" } else { "rolled" };
        if legal_turns(&self.position, self.turn, &self.dice).is_empty() {
            self.end_turn();
            self.message = format!("{side} {rolled} {first} and {second} and cannot move.");
            if self.mode == Mode::PassAndPlay {
                self.message = format!("{} {}", self.message, self.to_roll());
            }
        } else {
            self.message = format!(
                "{side} {rolled} {first} and {second}. {}",
                if self.next_moves().len() == 1 {
                    "Only one move is legal. Tap its checker."
                } else {
                    "Tap a checker to pick it up."
                }
            );
        }
    }

    fn next_moves(&self) -> Vec<Move> {
        let mut moves = Vec::new();
        for sequence in legal_turns(&self.position, self.turn, &self.dice) {
            if !moves.contains(&sequence[0]) {
                moves.push(sequence[0]);
            }
        }
        moves
    }

    fn displayed_moves(&self) -> Vec<Move> {
        let selected = self.selected.map(|selected| match selected {
            Selected::Bar => None,
            Selected::Point(point) => Some(point),
        });
        self.next_moves()
            .into_iter()
            .filter(|play| selected.is_none_or(|from| play.from == from))
            .collect()
    }

    fn end_turn(&mut self) {
        self.record_turn();
        self.dice.clear();
        self.selected = None;
        self.history.clear();
        self.turn = self.turn.other();
    }

    /// Writes down what this side just did, in the notation a board uses.
    ///
    /// Somebody who looks up from the panel and back again has no way to tell
    /// whether the checker on the five point arrived now or three turns ago.
    /// Two lines of "White 8/5 6/5" answer that without replaying anything.
    fn record_turn(&mut self) {
        let plays = std::mem::take(&mut self.turn_plays);
        self.last_turn = Some(LastTurn {
            player: self.turn,
            plays: plays.clone(),
        });
        if plays.is_empty() {
            return;
        }
        let line = format!("{} {}", self.side(self.turn), self.written(&plays));
        self.note(&line);
    }

    /// Adds a line to the record, keeping only the last few.
    fn note(&mut self, line: &str) {
        self.played.push(line.chars().take(64).collect());
        while self.played.len() > HISTORY {
            self.played.remove(0);
        }
    }

    /// A turn in the numbers printed beside the points: "13/8 13/10".
    fn written(&self, plays: &[Move]) -> String {
        let viewer = self.viewer();
        plays
            .iter()
            .map(|play| notation(viewer, *play))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn select(&mut self, from: Option<usize>) {
        if self.dice.is_empty() {
            self.message = "Tap Roll first.".into();
            return;
        }
        if self.next_moves().iter().any(|play| play.from == from) {
            self.selected = Some(match from {
                Some(point) => Selected::Point(point),
                None => Selected::Bar,
            });
            let off = self.displayed_moves().iter().any(|play| play.to.is_none());
            let points = self.displayed_moves().iter().any(|play| play.to.is_some());
            self.message = match (points, off) {
                (true, true) => "Tap a marked point, or Off to bear it off.",
                (false, true) => "Tap Off to bear this checker off.",
                _ if from.is_none() => "Tap a marked point to bring the checker in.",
                _ => "Tap a marked point to move it there.",
            }
            .into();
        } else if from.is_none() {
            self.message = "You have no checker on the bar.".into();
        } else if self.position.bar[self.turn.index()] > 0 {
            self.message = "A checker on the bar has to come in first. Tap Bar.".into();
        } else if from.is_some_and(|point| self.position.points[point].signum() == self.turn.sign())
        {
            self.message = "That checker cannot move with this roll.".into();
        } else {
            self.message = "Tap a point with one of your checkers on it.".into();
        }
    }

    /// A tap on a point: pick up, put down, or move, depending on what is in
    /// hand.
    ///
    /// Tapping the checker in hand puts it back, and tapping another checker
    /// that can move picks that one up instead, so a wrong pick never needs
    /// Undo. A point that is both open to the checker in hand and holds one of
    /// the player's own movable checkers takes the move, which is what the
    /// mark on it promised.
    fn tap_point(&mut self, point: usize) {
        let held = match self.selected {
            Some(Selected::Point(held)) => Some(Some(held)),
            Some(Selected::Bar) => Some(None),
            None => None,
        };
        if held == Some(Some(point)) {
            self.selected = None;
            self.message = "Checker put back. Tap a checker to pick it up.".into();
            return;
        }
        if let Some(from) = held {
            let open = self
                .next_moves()
                .iter()
                .any(|play| play.from == from && play.to == Some(point));
            if open {
                self.move_to(Some(point));
                return;
            }
            if self
                .next_moves()
                .iter()
                .any(|play| play.from == Some(point))
            {
                self.select(Some(point));
                return;
            }
            self.message =
                "That point is closed to this checker. Tap a marked point, or the checker again to put it back."
                    .into();
            return;
        }
        self.select(Some(point));
    }

    fn move_to(&mut self, to: Option<usize>) {
        let Some(selected) = self.selected else {
            self.message = if to.is_none() {
                "Tap the checker to bear off first, then Off."
            } else {
                "Tap a checker first."
            }
            .into();
            return;
        };
        let from = match selected {
            Selected::Bar => None,
            Selected::Point(point) => Some(point),
        };
        let Some(play) = self
            .next_moves()
            .into_iter()
            .find(|play| play.from == from && play.to == to)
        else {
            self.message = if to.is_none() {
                "This checker cannot bear off with this roll. Tap a marked point."
            } else {
                "Tap a marked point."
            }
            .into();
            return;
        };
        self.history.push(self.snapshot());
        self.turn_plays.push(play);
        self.position.apply(self.turn, play);
        let index = self
            .dice
            .iter()
            .position(|die| *die == play.die)
            .expect("legal move has a remaining die");
        self.dice.remove(index);
        self.selected = None;
        if self.position.won(self.turn) {
            self.record_turn();
            self.finish_game(self.turn);
        } else if self.dice.is_empty()
            || legal_turns(&self.position, self.turn, &self.dice).is_empty()
        {
            let mover = self.turn;
            self.end_turn();
            self.message = match self.mode {
                Mode::PassAndPlay => format!(
                    "{} moves are marked on the board. Pass the reader to {}.",
                    possessive(self.side(mover)),
                    self.side(self.turn)
                ),
                Mode::Solo => format!(
                    "{} moves are marked on the board. {}",
                    possessive(self.side(mover)),
                    self.to_roll()
                ),
            };
        } else {
            self.message = format!("{} left to play.", dice_words(&self.dice));
        }
    }

    fn offer_double(&mut self) {
        if self.phase != Phase::Playing || !self.dice.is_empty() {
            self.message = "Double before you roll, not after.".into();
        } else if self.opening {
            self.message = "Nobody can double before the opening roll.".into();
        } else if self.crawford_active {
            self.message = "Nobody may double in the Crawford game.".into();
        } else if self.cube >= MAX_CUBE {
            self.message = format!("The cube is already at its maximum of {MAX_CUBE}.");
        } else if self.cube_owner.is_some_and(|owner| owner != self.turn) {
            self.message = "Only the cube owner may double.".into();
        } else {
            self.phase = Phase::ConfirmDouble;
        }
    }

    fn confirm_double(&mut self) {
        self.phase = Phase::Offered(self.turn);
        self.message = format!("{} offers the cube at {}.", self.turn.name(), self.cube * 2);
    }

    fn take(&mut self, offered: Player) {
        self.cube = self.cube.saturating_mul(2).min(MAX_CUBE);
        self.cube_owner = Some(offered.other());
        self.phase = Phase::Playing;
        self.message = format!(
            "{} takes. {} to roll.",
            offered.other().name(),
            self.turn.name()
        );
    }

    fn drop_cube(&mut self, offered: Player) {
        self.score[offered.index()] = self.score[offered.index()].saturating_add(self.cube);
        let worth = points(self.cube);
        self.after_score(offered);
        self.message = format!(
            "{} dropped the cube. The game was worth {worth}.",
            self.side(offered.other())
        );
        self.note(&format!(
            "{} won a game worth {worth} on a drop.",
            self.side(offered)
        ));
    }

    fn finish_game(&mut self, winner: Player) {
        let earned = self.cube.saturating_mul(self.position.points_worth(winner));
        self.score[winner.index()] = self.score[winner.index()].saturating_add(earned);
        self.after_score(winner);
        self.message = format!("The game was worth {}.", points(earned));
        // The record runs on from one game into the next, so it says where
        // one ended.
        self.note(&format!(
            "{} won a game worth {}.",
            self.side(winner),
            points(earned)
        ));
    }

    fn after_score(&mut self, winner: Player) {
        self.dice.clear();
        self.selected = None;
        self.history.clear();
        if self.score[winner.index()] >= self.match_to {
            self.phase = Phase::MatchOver(winner);
        } else {
            self.phase = Phase::GameOver(winner);
        }
    }

    fn start_game(&mut self) {
        self.crawford_active = self.match_to > 1
            && !self.crawford_used
            && self
                .score
                .iter()
                .any(|score| score.saturating_add(1) == self.match_to);
        self.crawford_used |= self.crawford_active;
        self.opening = true;
        self.position = Position::initial();
        self.turn = Player::White;
        self.dice.clear();
        self.selected = None;
        self.cube = 1;
        self.cube_owner = None;
        self.phase = Phase::Playing;
        self.history.clear();
        self.turn_plays.clear();
        self.last_turn = None;
        self.message = if self.crawford_active {
            "Crawford game: nobody may double. Tap Roll to begin.".into()
        } else {
            opening_message(self.mode)
        };
    }

    fn start_match(&mut self) {
        self.played.clear();
        self.score = [0; 2];
        self.crawford_used = false;
        self.start_game();
    }

    fn set_match_to(&mut self, match_to: u8) {
        self.match_to = match_to;
        self.start_match();
        self.message = format!(
            "This match now plays to {}. Tap Roll to begin.",
            points(match_to)
        );
    }

    /// Whether starting again would throw anything away.
    fn match_in_progress(&self) -> bool {
        let untouched = self.opening
            && self.score == [0; 2]
            && self.played.is_empty()
            && self.position == Position::initial();
        let over = matches!(self.phase, Phase::MatchOver(_));
        !over && !untouched
    }

    /// Plays the computer's whole turn, roll and all, the moment it is due.
    ///
    /// It used to wait for a "Computer move" tap every turn, which made the
    /// player do the computer's work and paint the panel twice for one turn.
    /// Now the player's last checker lands, the computer rolls and moves,
    /// and the next screen shows the result once, with the points it moved
    /// from and to marked and one line saying what it did.
    ///
    /// Returns whether anything happened.
    fn play_computer_turn(&mut self) -> bool {
        if !self.computer_due() {
            return false;
        }
        // The player may have passed for want of a move. Say so first, or the
        // computer's line would be the only news and the pass would vanish.
        let lead = match &self.last_turn {
            Some(LastTurn {
                player: Player::White,
                plays,
            }) if plays.is_empty() && self.dice.is_empty() => format!("{} ", self.message),
            _ => String::new(),
        };
        // Dice already on the table are the opening roll, which the player
        // threw and the computer won.
        let opened = !self.dice.is_empty();
        if self.dice.is_empty() {
            self.roll();
            if self.dice.is_empty() {
                // Either the dice could not be rolled, which the roll has
                // already said, or the computer could not move.
                if self.turn != Player::Black {
                    self.message = format!("{lead}{}", self.message);
                }
                return self.turn != Player::Black;
            }
        }
        let rolled = match self.dice.as_slice() {
            [die, _, _, _] => format!("rolled double {die}s"),
            dice => format!(
                "{} {}",
                if opened { "opened with" } else { "rolled" },
                dice_words(dice)
            ),
        };
        while !self.dice.is_empty() && self.phase == Phase::Playing {
            let mut choices = self.next_moves();
            choices.sort_by_key(|play| std::cmp::Reverse(self.score_after(*play)));
            let Some(play) = choices.first().copied() else {
                break;
            };
            self.selected = Some(match play.from {
                Some(point) => Selected::Point(point),
                None => Selected::Bar,
            });
            self.move_to(play.to);
        }
        let moved = self
            .last_turn
            .as_ref()
            .filter(|last| last.player == Player::Black)
            .map(|last| self.written(&last.plays))
            .unwrap_or_default();
        let done = format!("{lead}Computer {rolled} and played {moved}.");
        self.message = match self.phase {
            Phase::Playing => done,
            _ => format!("{done} {}", self.message),
        };
        true
    }

    fn score_after(&self, play: Move) -> i16 {
        let mut after = self.position.clone();
        after.apply(self.turn, play);
        let own = self.turn.index();
        let theirs = self.turn.other().index();
        let pip = after
            .points
            .iter()
            .enumerate()
            .map(|(point, checkers)| {
                let distance = match self.turn {
                    Player::White => i16::try_from(point + 1).expect("point fits i16"),
                    Player::Black => i16::try_from(POINTS - point).expect("point fits i16"),
                };
                distance * i16::from((*checkers * self.turn.sign()).max(0))
            })
            .sum::<i16>();
        i16::from(after.off[own]) * 30 - i16::from(after.bar[own]) * 24 - pip
            + i16::from(after.bar[theirs]) * 18
    }

    fn encode(&self) -> String {
        let points = self
            .position
            .points
            .iter()
            .map(i8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let dice = self
            .dice
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{points};{},{};{},{};{};{};{};{};{},{};{};{};{};{};{};{};{}",
            self.position.bar[0],
            self.position.bar[1],
            self.position.off[0],
            self.position.off[1],
            self.turn.index(),
            dice,
            self.cube,
            self.cube_owner.map_or(2, Player::index),
            self.score[0],
            self.score[1],
            self.match_to,
            u8::from(self.crawford_used),
            u8::from(self.crawford_active),
            self.mode as u8,
            self.played.join("|"),
            u8::from(self.opening),
            self.phase.encode()
        )
    }

    /// The save written before turns were recorded.
    ///
    /// Its twelfth field counted rolls, which nothing needs now. A match left
    /// half-played is worth more than a tidy decoder.
    fn decode_previous(text: &str) -> Option<Self> {
        let fields: Vec<_> = text.split(';').collect();
        if fields.len() != 15 || fields[12].parse::<u64>().is_err() {
            return None;
        }
        let mut carried: Vec<&str> = fields;
        carried[12] = "";
        Self::decode(&carried.join(";"))
    }

    fn decode(text: &str) -> Option<Self> {
        let fields: Vec<_> = text.split(';').collect();
        if fields.len() != 15 {
            return None;
        }
        let phase = Phase::decode(fields[14])?;
        let mut game = Self {
            position: saved_position(fields[0], fields[1], fields[2])?,
            turn: saved_player(fields[3])?,
            dice: saved_dice(fields[4])?,
            selected: None,
            history_page: 0,
            rules_page: 0,
            cube: fields[5].parse().ok()?,
            cube_owner: saved_owner(fields[6]).ok()?,
            score: saved_pair(fields[7])?,
            match_to: fields[8].parse().ok()?,
            crawford_used: saved_flag(fields[9])?,
            crawford_active: saved_flag(fields[10])?,
            mode: saved_mode(fields[11])?,
            dice_source: Dice::open(),
            played: saved_history(fields[12]),
            turn_plays: Vec::new(),
            last_turn: None,
            opening: saved_flag(fields[13])?,
            phase,
            history: Vec::new(),
            message: saved_message(phase),
            save: SaveState::Kept,
            initial_load: InitialLoad::Pending,
            view: View::Board,
            menu: Menu::default(),
        };
        if !saved_game_is_safe(&game) {
            return None;
        }
        if game.phase == Phase::Playing {
            // Somebody coming back to a match wants the next step, not only
            // the fact that it was saved.
            game.message = format!(
                "Resumed saved game. {}",
                if game.dice.is_empty() {
                    game.to_roll()
                } else {
                    format!(
                        "{} left to play. Tap a checker to pick it up.",
                        dice_words(&game.dice)
                    )
                }
            );
        }
        game.normalize_saved_turn();
        Some(game)
    }

    fn normalize_saved_turn(&mut self) {
        if self.phase == Phase::Playing
            && !self.dice.is_empty()
            && legal_turns(&self.position, self.turn, &self.dice).is_empty()
        {
            self.end_turn();
            self.message = format!("No legal play. {} to roll.", self.turn.name());
        }
    }
}

/// The record of finished turns, as it was written down.
///
/// Anything unreadable is simply not a record: the game is still playable
/// without one, and refusing to open a saved match over its history would cost
/// the position as well.
fn saved_history(field: &str) -> Vec<String> {
    field
        .split('|')
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(64).collect())
        .take(HISTORY)
        .collect()
}

fn saved_pair(field: &str) -> Option<[u8; 2]> {
    field
        .split(',')
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?
        .try_into()
        .ok()
}

fn saved_position(points: &str, bar: &str, off: &str) -> Option<Position> {
    let values: Vec<_> = points
        .split(',')
        .map(str::parse::<i8>)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.len() != POINTS {
        return None;
    }
    let mut saved = [0; POINTS];
    saved.copy_from_slice(&values);
    Some(Position {
        points: saved,
        bar: saved_pair(bar)?,
        off: saved_pair(off)?,
    })
}

fn saved_player(field: &str) -> Option<Player> {
    match field {
        "0" => Some(Player::White),
        "1" => Some(Player::Black),
        _ => None,
    }
}

fn saved_dice(field: &str) -> Option<Vec<u8>> {
    let dice = if field.is_empty() {
        Vec::new()
    } else {
        field
            .split(',')
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .ok()?
    };
    dice.iter().all(|die| (1..=6).contains(die)).then_some(dice)
}

fn saved_owner(field: &str) -> Result<Option<Player>, ()> {
    match field {
        "0" => Ok(Some(Player::White)),
        "1" => Ok(Some(Player::Black)),
        "2" => Ok(None),
        _ => Err(()),
    }
}

fn saved_flag(field: &str) -> Option<bool> {
    match field {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

fn saved_mode(field: &str) -> Option<Mode> {
    match field {
        "0" => Some(Mode::Solo),
        "1" => Some(Mode::PassAndPlay),
        _ => None,
    }
}

fn saved_game_is_safe(game: &Game) -> bool {
    let points_are_bounded = game
        .position
        .points
        .iter()
        .all(|count| i16::from(*count).unsigned_abs() <= u16::from(CHECKERS));
    let white_points = game
        .position
        .points
        .iter()
        .filter(|count| **count > 0)
        .map(|count| u16::from(count.unsigned_abs()))
        .sum::<u16>();
    let black_points = game
        .position
        .points
        .iter()
        .filter(|count| **count < 0)
        .map(|count| u16::from(count.unsigned_abs()))
        .sum::<u16>();
    let all_checkers_accounted_for = white_points
        + u16::from(game.position.bar[Player::White.index()])
        + u16::from(game.position.off[Player::White.index()])
        == u16::from(CHECKERS)
        && black_points
            + u16::from(game.position.bar[Player::Black.index()])
            + u16::from(game.position.off[Player::Black.index()])
            == u16::from(CHECKERS);
    let dice_are_reachable = game.dice.len() <= 4
        && (game.dice.len() < 3 || game.dice.iter().all(|die| *die == game.dice[0]));
    let board_is_finished = game.position.won(Player::White) || game.position.won(Player::Black);
    let opening_is_canonical = game.turn == Player::White
        && game.position == Position::initial()
        && game.cube == 1
        && game.cube_owner.is_none();
    if !points_are_bounded
        || !all_checkers_accounted_for
        || !dice_are_reachable
        || !game
            .position
            .bar
            .iter()
            .chain(game.position.off.iter())
            .all(|count| *count <= CHECKERS)
        || !matches!(game.match_to, 1 | 3 | 5 | 7)
        || !(1..=MAX_CUBE).contains(&game.cube)
        || !game.cube.is_power_of_two()
        || (game.cube == 1 && game.cube_owner.is_some())
        || (game.cube > 1 && game.cube_owner.is_none())
        || (game.crawford_active
            && (game.match_to == 1
                || !game.crawford_used
                || !game
                    .score
                    .iter()
                    .any(|score| score.saturating_add(1) == game.match_to)
                || game.cube != 1
                || game.cube_owner.is_some()))
        || (game.opening
            && (!game.dice.is_empty() || game.phase != Phase::Playing || !opening_is_canonical))
    {
        return false;
    }
    let match_finished = game.score.iter().any(|score| *score >= game.match_to);
    let may_offer_cube = game.cube_owner.is_none_or(|owner| owner == game.turn);
    match game.phase {
        Phase::MatchOver(winner) => {
            game.score[winner.index()] >= game.match_to && game.dice.is_empty()
        }
        Phase::GameOver(winner) => {
            game.score[winner.index()] < game.match_to && !match_finished && game.dice.is_empty()
        }
        Phase::Playing => !match_finished && !board_is_finished,
        Phase::ConfirmDouble | Phase::Offered(_) | Phase::ConfirmDrop(_) => {
            !match_finished
                && !board_is_finished
                && !game.opening
                && game.dice.is_empty()
                && !game.crawford_active
                && game.cube < MAX_CUBE
                && may_offer_cube
                && !(game.mode == Mode::Solo && game.turn == Player::Black)
                && match game.phase {
                    Phase::Offered(player) | Phase::ConfirmDrop(player) => player == game.turn,
                    Phase::ConfirmDouble => true,
                    _ => unreachable!("phase was matched above"),
                }
        }
    }
}

fn saved_message(phase: Phase) -> String {
    match phase {
        Phase::Playing => "Resumed saved game.".into(),
        Phase::ConfirmDouble => "Resume the pending double offer.".into(),
        Phase::Offered(player) => format!("{} still offers the cube.", player.name()),
        Phase::ConfirmDrop(player) => {
            format!("Confirm whether to drop {}'s double.", player.name())
        }
        Phase::GameOver(winner) => format!("{} won the saved game.", winner.name()),
        Phase::MatchOver(winner) => format!("{} won the saved match.", winner.name()),
    }
}

fn point_name(point: usize) -> String {
    format!("point-{point}")
}

/// The points in the order the board shows them: the top row left to right,
/// then the bottom row left to right.
fn board_order(game: &Game) -> Vec<usize> {
    let mut order = (12..POINTS).chain((0..12).rev()).collect::<Vec<_>>();
    if game.mode == Mode::PassAndPlay && game.turn == Player::Black {
        order.reverse();
    }
    order
}

#[cfg(test)]
fn point_slot(game: &Game, point: usize) -> (usize, usize) {
    let slot = board_order(game)
        .iter()
        .position(|shown| *shown == point)
        .expect("all points are shown");
    (slot / 12, slot % 12)
}

/// The number printed beside a point, counted from the home of whoever the
/// board is facing.
fn shown_number(game: &Game, point: usize) -> usize {
    match game.viewer() {
        Player::White => point + 1,
        Player::Black => POINTS - point,
    }
}

/// How a point's number is drawn.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mark {
    #[default]
    Plain,
    /// Boxed: a point the player may tap now, or one the last turn's checkers
    /// left.
    Open,
    /// Inverted: the checker in hand, or a point the last turn's checkers
    /// came to.
    Solid,
}

/// What the board points at, point by point.
///
/// With dice to play it shows what can move, and once a checker is in hand
/// where that checker can go. Between turns it shows the turn just played,
/// which is how the computer's move is visible at all: its whole turn lands
/// in one repaint.
fn point_marks(game: &Game) -> [Mark; POINTS] {
    let mut marks = [Mark::Plain; POINTS];
    if game.phase != Phase::Playing {
        return marks;
    }
    if !game.dice.is_empty() {
        if game.computer_due() {
            return marks;
        }
        let moves = game.displayed_moves();
        match game.selected {
            Some(selected) => {
                if let Selected::Point(point) = selected {
                    marks[point] = Mark::Solid;
                }
                for to in moves.iter().filter_map(|play| play.to) {
                    marks[to] = Mark::Open;
                }
            }
            None => {
                for from in moves.iter().filter_map(|play| play.from) {
                    marks[from] = Mark::Open;
                }
            }
        }
        return marks;
    }
    if let Some(last) = &game.last_turn {
        // A checker that moved twice, 13/8/5, passed through the 8 and did
        // not stay, so only where the turn started and ended are marked.
        let mut net = [0_i8; POINTS];
        for play in &last.plays {
            if let Some(from) = play.from {
                net[from] -= 1;
            }
            if let Some(to) = play.to {
                net[to] += 1;
            }
        }
        for (mark, change) in marks.iter_mut().zip(net) {
            *mark = match change.cmp(&0) {
                std::cmp::Ordering::Less => Mark::Open,
                std::cmp::Ordering::Greater => Mark::Solid,
                std::cmp::Ordering::Equal => Mark::Plain,
            };
        }
    }
    marks
}

/// The 24 points as the runtime's backgammon cells, every one a tap target.
///
/// The runtime draws a cell's checkers from its glyph and the count in its
/// label, up to five, and ignores the selection flag, so the selection is
/// carried for any runtime that learns to draw it and the marks the player
/// needs now are in the numbered strips. An empty point the checker in hand
/// may go to carries the legal-move ring, which the runtime does draw, where
/// the checker would land.
fn board_cells(game: &Game) -> Vec<(String, String, Option<Glyph>, bool)> {
    let marks = point_marks(game);
    let holding = game.selected.is_some() && !game.dice.is_empty();
    board_order(game)
        .into_iter()
        .map(|point| {
            let checkers = game.position.points[point];
            let count = checkers.unsigned_abs();
            let number = shown_number(game, point);
            let mut label = if count == 0 {
                format!("Point {number} empty")
            } else {
                format!(
                    "Point {number} {} {count} checker{}",
                    if checkers > 0 { "white" } else { "black" },
                    if count == 1 { "" } else { "s" }
                )
            };
            let in_hand = holding && marks[point] == Mark::Solid;
            let open = holding && marks[point] == Mark::Open;
            if in_hand {
                label.push_str(", one in hand");
            }
            if open {
                label.push_str(", open");
            }
            let glyph = match checkers.signum() {
                1 => Some(Glyph::WhiteDisc),
                -1 => Some(Glyph::BlackDisc),
                _ if open => Some(Glyph::LegalPoint),
                _ => None,
            };
            (point_name(point), label, glyph, in_hand)
        })
        .collect()
}

/// A greyscale drawing in panel pixels, white unless drawn on.
struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        let size = usize::try_from(width).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
        Self {
            width: i32::try_from(width).unwrap_or(0),
            height: i32::try_from(height).unwrap_or(0),
            pixels: vec![PAPER; size],
        }
    }

    fn put(&mut self, x: i32, y: i32, tone: u8) {
        if (0..self.width).contains(&x) && (0..self.height).contains(&y) {
            let index = usize::try_from(y * self.width + x).expect("checked to be on the canvas");
            self.pixels[index] = tone;
        }
    }

    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, tone: u8) {
        for row in y..y + height {
            for column in x..x + width {
                self.put(column, row, tone);
            }
        }
    }

    fn stroke(&mut self, x: i32, y: i32, width: i32, height: i32, thickness: i32, tone: u8) {
        self.fill(x, y, width, thickness, tone);
        self.fill(x, y + height - thickness, width, thickness, tone);
        self.fill(x, y, thickness, height, tone);
        self.fill(x + width - thickness, y, thickness, height, tone);
    }

    fn disc(&mut self, centre_x: i32, centre_y: i32, radius: i32, tone: u8) {
        for y in -radius..=radius {
            for x in -radius..=radius {
                if x * x + y * y <= radius * radius {
                    self.put(centre_x + x, centre_y + y, tone);
                }
            }
        }
    }
}

/// The runtime's paper and ink, so the strips sit on the page rather than on
/// a grey card of their own.
const PAPER: u8 = 255;
const INK: u8 = 0;

/// The size of a drawn digit, in panel pixels.
///
/// The numbers were drawn on a board 960 pixels wide and shown at about 830
/// on a Clara, so a two pixel stroke came out at under two and the gap
/// between the digits of 13 closed up: 13 to 18 were read as 18 to 88. The
/// strips are drawn at the panel's own size now, and the gap stays open.
#[derive(Clone, Copy)]
struct Digits {
    width: i32,
    height: i32,
    stroke: i32,
    gap: i32,
}

const POINT_DIGITS: Digits = Digits {
    width: 14,
    height: 24,
    stroke: 3,
    gap: 6,
};
const COUNT_DIGITS: Digits = Digits {
    width: 9,
    height: 16,
    stroke: 2,
    gap: 3,
};
/// The room around a point's number, so a box drawn around it does not
/// touch the digits.
const MARK_PADDING: i32 = 6;
/// The width given to a point's number, boxed or not, so every number in a
/// row sits in the same place whatever it is marked with.
const MARK_WIDTH: i32 = 2 * POINT_DIGITS.width + POINT_DIGITS.gap + 2 * MARK_PADDING;
const COUNT_DIAMETER: i32 = 34;

fn seven_segment_digit(canvas: &mut Canvas, x: i32, y: i32, digit: u8, size: Digits, tone: u8) {
    const SEGMENTS: [[bool; 7]; 10] = [
        [true, true, true, true, true, true, false],
        [false, true, true, false, false, false, false],
        [true, true, false, true, true, false, true],
        [true, true, true, true, false, false, true],
        [false, true, true, false, false, true, true],
        [true, false, true, true, false, true, true],
        [true, false, true, true, true, true, true],
        [true, true, true, false, false, false, false],
        [true, true, true, true, true, true, true],
        [true, true, true, true, false, true, true],
    ];
    let Digits {
        width: wide,
        height: tall,
        stroke,
        ..
    } = size;
    let half = tall / 2;
    let segments = SEGMENTS[usize::from(digit.min(9))];
    // A one is the right-hand stroke alone, which left it hugging the next
    // cell over: 21 read as "2 I" and 11 as two strokes a gap apart. Drawn
    // down the middle of its cell instead, it sits where a printed 1 does.
    let x = if digit == 1 {
        x - (wide - stroke) / 2
    } else {
        x
    };
    let strokes = [
        (x + stroke, y, wide - 2 * stroke, stroke),
        (x + wide - stroke, y + stroke, stroke, half - stroke),
        (x + wide - stroke, y + half, stroke, half - stroke),
        (x + stroke, y + tall - stroke, wide - 2 * stroke, stroke),
        (x, y + half, stroke, half - stroke),
        (x, y + stroke, stroke, half - stroke),
        (x + stroke, y + half - stroke / 2, wide - 2 * stroke, stroke),
    ];
    for (on, (left, top, width, height)) in segments.into_iter().zip(strokes) {
        if on {
            canvas.fill(left, top, width, height, tone);
        }
    }
}

/// Draws a number of one or two digits centred on `centre`.
fn number(canvas: &mut Canvas, centre: i32, y: i32, value: u8, size: Digits, tone: u8) {
    let value = value.min(99);
    if value >= 10 {
        let left = centre - size.width - size.gap / 2;
        seven_segment_digit(canvas, left, y, value / 10, size, tone);
        seven_segment_digit(
            canvas,
            left + size.width + size.gap,
            y,
            value % 10,
            size,
            tone,
        );
    } else {
        seven_segment_digit(canvas, centre - size.width / 2, y, value, size, tone);
    }
}

/// The strip of numbers along one row of points, `top` or bottom.
///
/// Each number sits under its own cell, at the column the runtime gives it:
/// the board's width split twelve ways with no gaps. A point with more than
/// five checkers also gets a counter beside its number, drawn as a checker of
/// the same colour with the count on it, because the runtime's stack stops at
/// five and a home board of seven reads as five otherwise.
fn band_pixels(game: &Game, width: u32, top: bool) -> Vec<u8> {
    let mut canvas = Canvas::new(width, BAND_HEIGHT);
    let column = canvas.width / 12;
    let middle = canvas.height / 2;
    let order = board_order(game);
    let row = if top { &order[..12] } else { &order[12..] };
    let marks = point_marks(game);
    for (slot, point) in row.iter().enumerate() {
        let left = i32::try_from(slot).expect("twelve slots") * column;
        let checkers = game.position.points[*point];
        let count = checkers.unsigned_abs();
        let room = MARK_WIDTH + 3 + COUNT_DIAMETER;
        let counted = count > DRAWN_STACK && column >= room;
        let centre = if counted {
            left + (column - room) / 2 + MARK_WIDTH / 2
        } else {
            left + column / 2
        };
        let box_height = POINT_DIGITS.height + 2 * MARK_PADDING;
        let box_left = centre - MARK_WIDTH / 2;
        let box_top = middle - box_height / 2;
        let digits = match marks[*point] {
            Mark::Plain => INK,
            Mark::Open => {
                canvas.stroke(box_left, box_top, MARK_WIDTH, box_height, 3, INK);
                INK
            }
            Mark::Solid => {
                canvas.fill(box_left, box_top, MARK_WIDTH, box_height, INK);
                PAPER
            }
        };
        let shown = u8::try_from(shown_number(game, *point)).expect("point numbers fit");
        number(
            &mut canvas,
            centre,
            middle - POINT_DIGITS.height / 2,
            shown,
            POINT_DIGITS,
            digits,
        );
        if counted {
            let disc_x = box_left + MARK_WIDTH + 3 + COUNT_DIAMETER / 2;
            let radius = COUNT_DIAMETER / 2;
            let figures = if checkers > 0 {
                canvas.disc(disc_x, middle, radius, INK);
                canvas.disc(disc_x, middle, radius - 3, PAPER);
                INK
            } else {
                canvas.disc(disc_x, middle, radius, INK);
                PAPER
            };
            number(
                &mut canvas,
                disc_x,
                middle - COUNT_DIGITS.height / 2,
                count,
                COUNT_DIGITS,
                figures,
            );
        }
    }
    canvas.pixels
}

/// The two numbered strips, once they have been handed to the runtime.
#[derive(Clone, Copy, Debug)]
struct Bands {
    top: TilePicture,
    bottom: TilePicture,
}

#[cfg(test)]
fn screen(game: &Game, bands: Option<Bands>) -> Screen {
    screen_for(game, bands, &Context::default())
}

fn screen_for(game: &Game, bands: Option<Bands>, context: &Context) -> Screen {
    match game.view {
        View::History => return history_screen(game, context),
        View::NewMatch => {
            return ScreenBuilder::new("backgammon-new-match")
                .top_bar("Backgammon")
                .owns_back(true)
                .confirmation(
                    "Start a new game?",
                    "This ends the match in progress and clears its score and turn history. It cannot be undone.",
                    kobo_sdk::DialogAction::new("confirm-new-match", "Start new game"),
                    kobo_sdk::DialogAction::new("cancel-new-match", "Keep playing"),
                )
                .build()
        }
        View::Match => return match_screen(game),
        View::Help => return rules_screen(game, context),
        View::Board => {}
    }
    match game.phase {
        Phase::ConfirmDouble => ScreenBuilder::new("backgammon-double")
            .top_bar("Backgammon")
            .secondary(format!("Offer the cube at {}?", game.cube * 2))
            .secondary("The opponent may take or drop. This cannot be undone.")
            .grid(
                2,
                false,
                [
                    ("confirm-double", "Offer double"),
                    ("cancel-double", "Cancel"),
                ],
            )
            .build(),
        Phase::Offered(player) => ScreenBuilder::new("backgammon-cube")
            .top_bar("Backgammon")
            .secondary(format!(
                "{} offers the cube at {}.",
                player.name(),
                game.cube * 2
            ))
            .secondary(format!(
                "{} may take and play on for {}, or drop and give up {} now.",
                player.other().name(),
                points(game.cube * 2),
                points(game.cube)
            ))
            .grid(2, false, [("take", "Take"), ("drop", "Drop")])
            .build(),
        Phase::ConfirmDrop(player) => ScreenBuilder::new("backgammon-drop")
            .top_bar("Backgammon")
            .secondary(format!("Drop {}'s double?", player.name()))
            .secondary("This ends the game and cannot be undone.")
            .grid(
                2,
                false,
                [("confirm-drop", "Drop cube"), ("cancel-drop", "Cancel")],
            )
            .build(),
        Phase::GameOver(winner) => result_screen(
            game,
            format!("{} won this game.", game.side(winner)),
            ("next-game", "Next game"),
        ),
        Phase::MatchOver(winner) => result_screen(
            game,
            format!(
                "{} won the match {}–{}.",
                game.side(winner),
                game.score[winner.index()],
                game.score[winner.other().index()]
            ),
            (game::NEW_GAME, game::NEW_GAME_LABEL),
        ),
        Phase::Playing => fitted_playing_screen(game, bands, context),
    }
}

/// Between games: who won, the score, and the one thing to do next.
///
/// It keeps the game's top bar and Undo in their places, so the rules and the
/// menu are where they were a move ago. At the end of a match the next thing
/// is New game itself, the same action under the same name as in the menu.
fn result_screen(game: &Game, heading: String, next: (&str, &str)) -> Screen {
    ScreenBuilder::new("backgammon-result")
        .game_top_bar("Backgammon", game.menu, MORE_ITEMS)
        .heading(heading)
        .secondary(score_line(game))
        .secondary(if game.save == SaveState::Refused {
            SAVE_FAILED
        } else {
            &game.message
        })
        .game_actions(true, [next])
        .build()
}

/// How much of the status under the board to keep when the text is large.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Detail {
    Full,
    Short,
    Dice,
    Bare,
}

/// The board screen with as much of the status under the board as fits.
///
/// Everything above the board is one line that never changes height, so the
/// board sits in the same place, at the same size, for the whole game: it
/// used to give up height to whatever the notice and the move list needed on
/// each roll, and changed size under the player's eyes. What does not fit at
/// a large text size is dropped from the bottom of the status instead.
fn fitted_playing_screen(game: &Game, bands: Option<Bands>, context: &Context) -> Screen {
    let metrics = context.metrics();
    let chrome = kobo_sdk::Chrome::measuring(true);
    for detail in [Detail::Full, Detail::Short, Detail::Dice] {
        let candidate = playing_screen(game, bands, detail);
        if candidate.diagnostics(&metrics, &chrome).has_errors() {
            continue;
        }
        // A block of facts that runs out of room loses its last rows without
        // a diagnostic, so the score simply vanished at large text. Every
        // value has to be on the panel for this level of detail to count.
        let layout = candidate.layout_with(&metrics, &chrome);
        let shown = |value: &String| {
            layout
                .nodes
                .iter()
                .any(|node| node.text_lines.join(" ") == *value)
        };
        if facts_for(game, detail)
            .iter()
            .all(|(_, value)| shown(value))
        {
            return candidate;
        }
    }
    playing_screen(game, bands, Detail::Bare)
}

/// The status under the board at each level of detail.
fn facts_for(game: &Game, detail: Detail) -> Vec<(&'static str, String)> {
    match detail {
        Detail::Full => vec![
            ("Dice", dice_line(game)),
            ("Cube", cube_line(game)),
            ("Score", score_line(game)),
        ],
        Detail::Short => vec![
            ("Dice", dice_line(game)),
            (
                "Score",
                format!(
                    "{}–{} to {} · cube {}",
                    game.score[0], game.score[1], game.match_to, game.cube
                ),
            ),
        ],
        Detail::Dice => vec![("Dice", dice_line(game))],
        Detail::Bare => Vec::new(),
    }
}

fn playing_screen(game: &Game, bands: Option<Bands>, detail: Detail) -> Screen {
    let mut screen = ScreenBuilder::new("backgammon")
        .game_top_bar("Backgammon", game.menu, MORE_ITEMS)
        // Whose move it is, first and on its own line. It used to be the first
        // clause of a line that also carried the score, the cube and the dice,
        // which is the one thing somebody picking the reader up again needs to
        // read at a glance.
        .section(turn_line(game));
    if let Some(bands) = bands {
        screen = screen.full_bleed_picture(bands.top, 10);
    }
    screen = screen.board_with_selection(12, board_cells(game));
    if let Some(bands) = bands {
        screen = screen.full_bleed_picture(bands.bottom, 10);
    }
    let rolled = !game.dice.is_empty() && !game.computer_due();
    let bar_open = rolled
        && game.selected.is_none()
        && game.next_moves().iter().any(|play| play.from.is_none());
    let bar_held = rolled && game.selected == Some(Selected::Bar);
    let off_open = rolled
        && game.selected.is_some()
        && game.displayed_moves().iter().any(|play| play.to.is_none());
    screen = screen
        .grid_with_selection(
            2,
            false,
            [
                (
                    "bar",
                    checker_counter("Bar", game.position.bar),
                    bar_open || bar_held,
                ),
                ("off", checker_counter("Off", game.position.off), off_open),
            ],
        )
        .secondary(if game.save == SaveState::Refused {
            SAVE_FAILED
        } else {
            &game.message
        });
    // The dice and the cube in words. The board draws neither: the runtime's
    // bar is a strip too narrow for a die.
    screen = screen.facts(facts_for(game, detail));
    screen
        .game_actions(true, [("roll", "Roll"), ("double", "Double")])
        .build()
}

/// Whose move it is, said the way the reader in front of the panel would.
fn turn_line(game: &Game) -> String {
    if game.opening {
        return "Opening roll".to_owned();
    }
    match (game.mode, game.turn) {
        (Mode::Solo, Player::White) => "Your move".to_owned(),
        (Mode::Solo, Player::Black) => "Computer to play".to_owned(),
        (Mode::PassAndPlay, player) => format!("{} to play", player.name()),
    }
}

/// Dice as a sentence says them: "5", "5 and 3", "4, 4 and 4".
fn dice_words(dice: &[u8]) -> String {
    let words = dice.iter().map(u8::to_string).collect::<Vec<_>>();
    match words.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

fn dice_line(game: &Game) -> String {
    if game.dice.is_empty() {
        return "Not rolled yet".to_owned();
    }
    dice_words(&game.dice)
}

fn cube_line(game: &Game) -> String {
    match game.cube_owner {
        None if game.crawford_active => format!("{} · no doubling this game", game.cube),
        None => format!("{} · either side may double", game.cube),
        Some(owner) => format!("{} · {} owns it", game.cube, owner.name()),
    }
}

fn score_pair(game: &Game) -> String {
    format!(
        "{} {} · {} {}",
        game.side(Player::White),
        game.score[0],
        game.side(Player::Black),
        game.score[1]
    )
}

fn score_line(game: &Game) -> String {
    format!("{} · first to {}", score_pair(game), game.match_to)
}

fn points(value: u8) -> String {
    if value == 1 {
        "1 point".to_owned()
    } else {
        format!("{value} points")
    }
}

fn possessive(side: &str) -> String {
    if side == "You" {
        "Your".to_owned()
    } else {
        format!("{side}'s")
    }
}

/// The rules, one paragraph to a topic.
const RULES_TEXT: [&str; 6] = [
    "Tap Roll. Then tap one of your checkers to pick it up, and tap a marked point to move it there. Tap the checker again to put it back.",
    "A boxed number is a point you can tap now, and a solid one is the checker in hand. Between turns they show the last turn: boxed where its checkers left, solid where they landed.",
    "Landing on a lone opposing checker sends it to the bar. Checkers on the bar have to come back in first: tap Bar, then a marked point.",
    "Bring all 15 checkers into your home board, the points numbered 1 to 6, then tap a checker and Off to bear it off. The first to bear off all 15 wins.",
    "Before you roll, Double offers to play on for twice the points. Your opponent may take or drop.",
    "Against the computer you play White and the computer plays Black. It rolls and moves as soon as your turn ends.",
];

fn rules_pages(context: &Context) -> Vec<Vec<String>> {
    context.paginate(&RULES_TEXT.join("\n\n"), true)
}

/// The rules, in as many pages as the text size needs. A page that ran off
/// the bottom of the panel lost its last paragraph without a word.
fn rules_screen(game: &Game, context: &Context) -> Screen {
    let pages = rules_pages(context);
    let page = game.rules_page.min(pages.len().saturating_sub(1));
    let mut screen = ScreenBuilder::new("backgammon-rules")
        .top_bar(game::RULES_LABEL)
        .owns_back(true);
    for paragraph in &pages[page] {
        screen = screen.text(paragraph);
    }
    if pages.len() > 1 {
        screen = screen
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            )
            .action_bar([
                ("rules-previous", "Previous"),
                ("rules-next", "Next"),
                ("close-help", "Play"),
            ]);
    } else {
        screen = screen.bottom_action("close-help", "Play");
    }
    screen.build()
}

/// The match: who is playing, how long it runs, and where the dice come from.
///
/// A setting that cannot change right now is stated rather than offered: a
/// button that did nothing once the match had started was a control with no
/// answer.
fn match_screen(game: &Game) -> Screen {
    let between_turns = game.phase == Phase::Playing && game.dice.is_empty();
    let fresh = game.score == [0; 2] && game.opening && game.phase == Phase::Playing;
    let mut screen = ScreenBuilder::new("backgammon-match")
        .top_bar("Match settings")
        .owns_back(true)
        .facts([("Score", score_pair(game))]);
    if let Some(provenance) = game.dice_source.provenance() {
        // A recorded game that looks like chance and is not would be a lie
        // about the dice, so a seeded run says so wherever the match is shown.
        screen = screen.secondary(provenance);
    }
    screen = screen.section("Players");
    screen = if between_turns {
        screen.segmented(
            usize::from(game.mode == Mode::PassAndPlay),
            [
                ("mode-solo", "Against the computer"),
                ("mode-pass", "Two players"),
            ],
        )
    } else {
        screen
            .text(match game.mode {
                Mode::Solo => "Against the computer.",
                Mode::PassAndPlay => "Two players, passing the reader.",
            })
            .secondary("This can change once the dice in play are used.")
    };
    screen = screen.section("Points to win");
    screen = if fresh {
        screen.segmented(
            [1, 3, 5, 7]
                .iter()
                .position(|length| *length == game.match_to)
                .unwrap_or(usize::MAX),
            [
                ("length-1", "1"),
                ("length-3", "3"),
                ("length-5", "5"),
                ("length-7", "7"),
            ],
        )
    } else {
        screen
            .text(format!("This match plays to {}.", points(game.match_to)))
            .secondary("Start a new game from the menu to choose another length.")
    };
    screen.bottom_action("close-match", "Play").build()
}

fn history_pages(game: &Game, context: &Context) -> Vec<Vec<String>> {
    let text = if game.played.is_empty() {
        "Nothing played yet.".to_owned()
    } else {
        game.played
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    context.paginate(&text, false)
}

fn history_screen(game: &Game, context: &Context) -> Screen {
    let pages = history_pages(game, context);
    let page = game.history_page.min(pages.len().saturating_sub(1));
    let mut builder = ScreenBuilder::new("backgammon-history")
        .top_bar("Turn history")
        .owns_back(true);
    for text in &pages[page] {
        builder = builder.text(text);
    }
    if pages.len() > 1 {
        builder = builder
            .page_turns("history-previous", "history-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
    }
    builder.build()
}

/// One move as a board writes it: "8/5", "bar/20", "6/off", counted from the
/// home of `viewer`.
fn notation(viewer: Player, play: Move) -> String {
    let point = |index: usize| match viewer {
        Player::White => index + 1,
        Player::Black => POINTS - index,
    };
    let from = play
        .from
        .map_or_else(|| "bar".to_owned(), |index| point(index).to_string());
    let to = play
        .to
        .map_or_else(|| "off".to_owned(), |index| point(index).to_string());
    format!("{from}/{to}")
}

fn checker_counter(label: &str, count: [u8; 2]) -> String {
    match count {
        [0, 0] => format!("{label} · none"),
        [white, 0] => format!("{label} · White {white}"),
        [0, black] => format!("{label} · Black {black}"),
        [white, black] => format!("{label} · White {white} · Black {black}"),
    }
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SAVE);
        context.store().load(OLD_SAVE);
        self.show(context);
    }
    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Help {
            let last = rules_pages(context).len().saturating_sub(1);
            let page = self.rules_page.min(last);
            self.rules_page = if forward {
                (page + 1).min(last)
            } else {
                page.saturating_sub(1)
            };
            self.show(context);
        }
        if self.view == View::History {
            let last = history_pages(self, context).len().saturating_sub(1);
            let page = self.history_page.min(last);
            self.history_page = if forward {
                (page + 1).min(last)
            } else {
                page.saturating_sub(1)
            };
            self.show(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::History
            && (action == action_id("history-next") || action == action_id("history-previous"))
        {
            self.on_page_turn(context, action == action_id("history-next"));
            return;
        }
        if self.view == View::Help
            && (action == action_id("rules-next") || action == action_id("rules-previous"))
        {
            self.on_page_turn(context, action == action_id("rules-next"));
            return;
        }
        if let Some(persisted) = self.apply_action(action) {
            if persisted {
                context.store().save(SAVE, self.encode());
            }
            self.show(context);
        }
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match &result {
            StoreResult::Denied(_) => {
                self.save = SaveState::Refused;
                self.show(context);
                return;
            }
            StoreResult::Saved { .. } if self.save == SaveState::Refused => {
                self.save = SaveState::Kept;
                self.show(context);
                return;
            }
            _ => {}
        }
        if let StoreResult::Loaded { key, value } = result {
            if key == OLD_SAVE {
                // Only when this session has nothing newer to show for itself.
                if self.initial_load == InitialLoad::Pending {
                    let carried = value
                        .as_deref()
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .and_then(Self::decode_previous);
                    if let Some(mut carried) = carried {
                        carried.initial_load = InitialLoad::Settled;
                        *self = carried;
                        self.play_computer_turn();
                        context.store().save(SAVE, self.encode());
                        self.show(context);
                    }
                }
                return;
            }
            if key == SAVE {
                let normalized = value
                    .as_deref()
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .is_some_and(|encoded| {
                        Self::decode(encoded).is_some_and(|saved| saved.encode() != encoded)
                    });
                if self.apply_initial_load(value) {
                    // A match saved while the computer was due, by a version
                    // that waited for a tap, is played on from where it was.
                    let played = self.play_computer_turn();
                    if normalized || played {
                        context.store().save(SAVE, self.encode());
                    }
                    self.show(context);
                }
            }
        }
    }
}

impl Game {
    fn apply_action(&mut self, action: ActionId) -> Option<bool> {
        let before = self.encode();
        game_action(self, action)?;
        self.play_computer_turn();
        let persisted = self.encode() != before;
        if persisted {
            self.initial_load = InitialLoad::Settled;
        }
        Some(persisted)
    }

    fn apply_initial_load(&mut self, value: Option<Vec<u8>>) -> bool {
        if self.initial_load == InitialLoad::Settled {
            return false;
        }
        self.initial_load = InitialLoad::Settled;
        if let Some(value) = value {
            if let Ok(text) = String::from_utf8(value) {
                if let Some(mut saved) = Self::decode(&text) {
                    saved.initial_load = InitialLoad::Settled;
                    *self = saved;
                }
            }
        }
        true
    }

    fn show(&self, context: &mut Context) {
        let bands = if self.phase == Phase::Playing && self.view == View::Board {
            let width = u32::try_from(context.metrics().width).unwrap_or(0);
            let top =
                context.put_picture(TOP_BAND, width, BAND_HEIGHT, band_pixels(self, width, true));
            let bottom = context.put_picture(
                BOTTOM_BAND,
                width,
                BAND_HEIGHT,
                band_pixels(self, width, false),
            );
            top.zip(bottom).map(|(top, bottom)| Bands { top, bottom })
        } else {
            None
        };
        context.set_screen(screen_for(self, bands, context));
    }
}

/// The screens off the board: the match settings, the rules, the history and
/// the question before a new game.
///
/// `Some` when the tap belonged to one of them, which is the caller's cue to
/// stop: the same word means something different on each of them.
fn view_action(game: &mut Game, action: ActionId) -> Option<()> {
    match game.view {
        View::History => {
            if action == ActionId::BACK {
                game.view = View::Board;
            }
            Some(())
        }
        View::NewMatch => {
            if action == action_id("confirm-new-match") {
                game.start_match();
                game.view = View::Board;
            } else if action == action_id("cancel-new-match") || action == ActionId::BACK {
                game.view = View::Board;
            }
            Some(())
        }
        View::Help => {
            if action == action_id("close-help") || action == ActionId::BACK {
                game.view = View::Board;
            }
            Some(())
        }
        View::Match => {
            match_action(game, action);
            Some(())
        }
        View::Board if action == action_id("match-settings") => {
            game.view = View::Match;
            Some(())
        }
        View::Board if action == action_id("turn-history") => {
            game.history_page = 0;
            game.view = View::History;
            Some(())
        }
        View::Board => None,
    }
}

fn match_action(game: &mut Game, action: ActionId) {
    if action == action_id("close-match") || action == ActionId::BACK {
        game.view = View::Board;
        return;
    }
    let between_turns = game.phase == Phase::Playing && game.dice.is_empty();
    let fresh = game.score == [0; 2] && game.opening && game.phase == Phase::Playing;
    if between_turns {
        for (name, mode) in [("mode-solo", Mode::Solo), ("mode-pass", Mode::PassAndPlay)] {
            if action == action_id(name) && game.mode != mode {
                game.mode = mode;
                game.message = if game.opening {
                    opening_message(mode)
                } else {
                    game.to_roll()
                };
            }
        }
    }
    if fresh {
        for length in [1, 3, 5, 7] {
            if action == action_id(&format!("length-{length}")) && game.match_to != length {
                game.set_match_to(length);
            }
        }
    }
}

/// What to do first, for each way of playing.
fn opening_message(mode: Mode) -> String {
    match mode {
        Mode::Solo => "You play White against the computer. Tap Roll to begin.".to_owned(),
        Mode::PassAndPlay => "Tap Roll. The higher die moves first.".to_owned(),
    }
}

/// The shared controls: Undo, Rules and the menu with New game first.
fn chrome_action(game: &mut Game, action: ActionId) -> Option<()> {
    if game.view != View::Board {
        return None;
    }
    match game.menu.read(action)? {
        Tap::Menu => {}
        Tap::Undo => game.undo(),
        Tap::Rules => {
            game.rules_page = 0;
            game.view = View::Help;
        }
        Tap::NewGame => {
            if game.match_in_progress() {
                game.view = View::NewMatch;
            } else {
                game.start_match();
            }
        }
    }
    Some(())
}

fn game_action(game: &mut Game, action: ActionId) -> Option<()> {
    if chrome_action(game, action).is_some() {
        return Some(());
    }
    if matches!(
        game.view,
        View::Help | View::NewMatch | View::History | View::Match
    ) {
        let previous = (game.view, game.encode(), game.mode);
        view_action(game, action);
        return ((game.view, game.encode(), game.mode) != previous).then_some(());
    }
    if view_action(game, action).is_some() {
        return Some(());
    }
    if game.mode == Mode::Solo
        && game.turn == Player::Black
        && matches!(
            game.phase,
            Phase::Playing | Phase::ConfirmDouble | Phase::Offered(_) | Phase::ConfirmDrop(_)
        )
    {
        // The computer's turn is played the moment it is due, so a tap here
        // can only arrive while its dice cannot be rolled. Nothing on the
        // board is the player's to move.
        return None;
    }
    match game.phase {
        Phase::ConfirmDouble if action == action_id("confirm-double") => {
            game.confirm_double();
            if game.mode == Mode::Solo && game.turn == Player::White {
                game.take(Player::White);
                game.message = format!("Computer takes. {}", game.to_roll());
            }
        }
        Phase::ConfirmDouble if action == action_id("cancel-double") => game.phase = Phase::Playing,
        Phase::Offered(player) if action == action_id("take") => game.take(player),
        Phase::Offered(player) if action == action_id("drop") => {
            game.phase = Phase::ConfirmDrop(player);
        }
        Phase::ConfirmDrop(player) if action == action_id("confirm-drop") => game.drop_cube(player),
        Phase::ConfirmDrop(player) if action == action_id("cancel-drop") => {
            game.phase = Phase::Offered(player);
        }
        Phase::GameOver(_) if action == action_id("next-game") => game.start_game(),
        Phase::Playing if action == action_id("roll") => {
            if game.dice.is_empty() {
                game.roll();
            } else {
                game.message = format!(
                    "The dice are already rolled. {} left to play.",
                    dice_words(&game.dice)
                );
            }
        }
        Phase::Playing if action == action_id("double") => game.offer_double(),
        Phase::Playing if action == action_id("bar") => {
            if game.selected == Some(Selected::Bar) {
                game.selected = None;
                game.message = "Checker put back on the bar.".into();
            } else {
                game.select(None);
            }
        }
        Phase::Playing if action == action_id("off") => game.move_to(None),
        Phase::Playing => {
            let point = (0..POINTS).find(|point| action == action_id(&point_name(*point)))?;
            game.tap_point(point);
        }
        _ => return None,
    }
    Some(())
}

fn main() -> ExitCode {
    match kobo_sdk::run("backgammon", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("backgammon: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, LayoutKind, CLARA_BW_METRICS};

    fn test_screen(game: &Game) -> Screen {
        // Match runtime typography even when a layout test runs on its own.
        static FONT: std::sync::Once = std::sync::Once::new();
        FONT.call_once(|| {
            let _runner = kobo_sdk::AppRunner::new(Game::default());
            assert!(kobo_ui::has_typesetter());
        });
        screen(game, Some(test_bands()))
    }

    /// The two numbered strips as the runtime would hold them on a Clara.
    fn test_bands() -> Bands {
        Bands {
            top: TilePicture::new(TOP_BAND, 1072, BAND_HEIGHT),
            bottom: TilePicture::new(BOTTOM_BAND, 1072, BAND_HEIGHT),
        }
    }

    /// Every line of text the screen draws, joined, for checking copy.
    fn drawn(screen: &Screen) -> String {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Where the runtime puts the board on a Clara.
    fn board_rect(screen: &Screen) -> kobo_ui::Rect {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::BackgammonBoard))
            .expect("the board is drawn")
            .rect
    }

    /// Taps whatever the screen draws at the middle of `name`'s control, the
    /// way a finger would, and returns what the tap reached.
    fn tap(game: &mut Game, name: &str) -> ActionId {
        let layout = test_screen(game).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let rect = layout
            .rect_of_action(action_id(name))
            .unwrap_or_else(|| panic!("{name} is not on the screen"));
        let hit = layout
            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
            .expect("the middle of a control answers a tap");
        game.apply_action(hit);
        hit
    }

    /// The pixel at the middle of a point's slot in its numbered strip.
    fn band_mark(game: &Game, point: usize) -> u8 {
        let (row, column) = point_slot(game, point);
        let pixels = band_pixels(game, 1072, row == 0);
        let x = column * (1072 / 12) + (1072 / 12) / 2;
        // Just inside the left edge of the box around the number, where a
        // boxed number has its frame and a solid one its fill.
        let x = x - usize::try_from(MARK_WIDTH / 2 - 1).expect("positive");
        let y = usize::try_from(BAND_HEIGHT / 2).expect("fits");
        pixels[y * 1072 + x]
    }

    fn position(points: &[(usize, i8)], white_bar: u8, black_bar: u8) -> Position {
        let mut result = Position {
            points: [0; POINTS],
            bar: [white_bar, black_bar],
            off: [0; 2],
        };
        for (point, count) in points {
            result.points[*point] = *count;
        }
        result
    }

    #[test]
    fn opening_position_has_fifteen_checkers_each() {
        let board = Position::initial();
        assert_eq!(
            board.points.iter().filter(|count| **count > 0).sum::<i8>(),
            15
        );
        assert_eq!(
            board
                .points
                .iter()
                .filter(|count| **count < 0)
                .map(|count| -*count)
                .sum::<i8>(),
            15
        );
    }

    #[test]
    fn bar_entry_is_mandatory_and_respects_blocks() {
        let board = position(&[(5, 1), (23, -2)], 1, 0);
        assert_eq!(board.single_moves(Player::White, 1), Vec::<Move>::new());
        assert_eq!(
            board.single_moves(Player::White, 6),
            vec![Move {
                from: None,
                to: Some(18),
                die: 6
            }]
        );
    }

    #[test]
    fn blocked_points_and_blots_follow_hitting_rules() {
        let mut board = position(&[(6, 1), (5, -2), (4, -1)], 0, 0);
        assert!(board.single_moves(Player::White, 1).is_empty());
        assert_eq!(board.single_moves(Player::White, 2)[0].to, Some(4));
        board.apply(Player::White, board.single_moves(Player::White, 2)[0]);
        assert_eq!(board.bar[Player::Black.index()], 1);
        assert_eq!(board.points[4], 1);
    }

    #[test]
    fn doubles_use_all_four_dice_when_possible() {
        let board = position(&[(23, 4)], 0, 0);
        let turns = legal_turns(&board, Player::White, &[3, 3, 3, 3]);
        assert!(!turns.is_empty());
        assert!(turns.iter().all(|turn| turn.len() == 4));
    }

    #[test]
    fn both_dice_and_higher_die_rule_are_enforced() {
        let board = position(&[(23, -2), (17, -2)], 1, 0);
        let turns = legal_turns(&board, Player::White, &[1, 6]);
        assert_eq!(
            turns,
            vec![vec![Move {
                from: None,
                to: Some(18),
                die: 6
            }]]
        );
        let open = position(&[(7, 1)], 0, 0);
        assert!(legal_turns(&open, Player::White, &[1, 2])
            .iter()
            .all(|turn| turn.len() == 2));
    }

    #[test]
    fn bearing_off_allows_exact_and_oversize_only_from_farthest_checker() {
        let board = position(&[(4, 1), (1, 1)], 0, 0);
        assert_eq!(
            board.single_moves(Player::White, 5),
            vec![Move {
                from: Some(4),
                to: None,
                die: 5
            }]
        );
        assert!(board
            .single_moves(Player::White, 6)
            .iter()
            .any(|play| play.from == Some(4) && play.to.is_none()));
        assert!(!board
            .single_moves(Player::White, 6)
            .iter()
            .any(|play| play.from == Some(1) && play.to.is_none()));
    }

    #[test]
    fn exact_bear_off_does_not_require_the_farthest_checker() {
        let white = position(&[(0, 1), (5, 1)], 0, 0);
        assert!(white
            .single_moves(Player::White, 1)
            .iter()
            .any(|play| play.from == Some(0) && play.to.is_none()));
        let black = position(&[(23, -1), (18, -1)], 0, 0);
        assert!(black
            .single_moves(Player::Black, 1)
            .iter()
            .any(|play| play.from == Some(23) && play.to.is_none()));
    }

    #[test]
    fn off_control_bears_off_a_selected_checker() {
        let mut game = Game {
            position: position(&[(0, 1)], 0, 0),
            dice: vec![1],
            selected: Some(Selected::Point(0)),
            ..Game::default()
        };
        game_action(&mut game, action_id("off"));
        assert_eq!(game.position.off[Player::White.index()], 1);
    }

    #[test]
    fn bar_control_selects_and_enters_a_checker() {
        let mut game = Game {
            position: position(&[], 1, 0),
            dice: vec![1],
            ..Game::default()
        };
        game_action(&mut game, action_id("bar"));
        assert_eq!(game.selected, Some(Selected::Bar));
        game_action(&mut game, action_id("point-23"));
        assert_eq!(game.position.bar[Player::White.index()], 0);
        assert_eq!(game.position.points[23], 1);
    }

    #[test]
    fn no_move_turn_switches_sides() {
        let mut game = Game {
            position: position(
                &[(23, 1), (22, -2), (21, -2), (20, -2), (19, -2), (18, -2)],
                0,
                1,
            ),
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        assert!(game.dice.is_empty());
        assert_eq!(game.turn, Player::Black);
    }

    #[test]
    fn opening_roll_rerolls_ties_and_awards_first_turn_to_higher_die() {
        // A pair decides nothing, so the opening throws it away and rolls on.
        let mut game = Game {
            dice_source: Dice::scripted([(4, 4), (2, 1)]),
            ..Game::default()
        };
        game.roll();
        assert!(!game.opening);
        assert_eq!(game.turn, Player::White);
        assert_eq!(game.dice, vec![2, 1]);
        assert!(game.message.starts_with("You opened with 2 and 1"));

        let mut black_first = Game {
            dice_source: Dice::scripted([(1, 6)]),
            ..Game::default()
        };
        black_first.roll();
        assert_eq!(black_first.turn, Player::Black);
    }

    #[test]
    fn turn_entry_requires_all_usable_dice_before_switching_players() {
        let mut game = Game {
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        game.select(Some(23));
        game.move_to(Some(22));
        assert_eq!(game.dice, vec![2]);
        assert_eq!(game.turn, Player::White);
        game.select(Some(22));
        game.move_to(Some(20));
        assert!(game.dice.is_empty());
        assert_eq!(game.turn, Player::Black);
    }

    #[test]
    fn cube_sequence_and_crawford_restriction_are_correct() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        game.offer_double();
        assert_eq!(game.phase, Phase::ConfirmDouble);
        game.confirm_double();
        game.take(Player::White);
        assert_eq!((game.cube, game.cube_owner), (2, Some(Player::Black)));
        game.crawford_active = true;
        game.offer_double();
        assert_eq!(game.phase, Phase::Playing);
        assert_eq!(game.message, "Nobody may double in the Crawford game.");
    }

    #[test]
    fn computer_accepts_a_solo_cube_offer_without_owner_input() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        game.offer_double();
        game_action(&mut game, action_id("confirm-double"));
        assert_eq!(game.phase, Phase::Playing);
        assert_eq!((game.cube, game.cube_owner), (2, Some(Player::Black)));
        assert_eq!(game.message, "Computer takes. Your roll.");
    }

    #[test]
    fn autosave_round_trips_position_and_match_state() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        game.position = position(&[(23, 11), (0, -9)], 1, 2);
        game.position.off = [3, 4];
        game.dice = vec![4, 2];
        game.score = [2, 3];
        game.mode = Mode::PassAndPlay;
        assert_eq!(
            Game::decode(&game.encode()).unwrap().encode(),
            game.encode()
        );
    }

    #[test]
    fn a_late_initial_load_cannot_overwrite_local_play() {
        let saved = Game::default().encode().into_bytes();
        let mut game = Game {
            score: [2, 1],
            initial_load: InitialLoad::Settled,
            ..Game::default()
        };
        assert!(!game.apply_initial_load(Some(saved)));
        assert_eq!(game.score, [2, 1]);
    }

    #[test]
    fn a_tap_that_changes_nothing_does_not_discard_the_pending_initial_load() {
        let mut game = Game::default();
        assert_eq!(
            game.apply_action(action_id("point-5")),
            Some(false),
            "a point tapped before the roll only says to roll"
        );
        assert_eq!(game.message, "Tap Roll first.");
        assert_eq!(game.initial_load, InitialLoad::Pending);
    }

    #[test]
    fn dropping_a_cube_names_the_winner() {
        let mut game = Game {
            opening: false,
            cube: 2,
            cube_owner: Some(Player::Black),
            ..Game::default()
        };
        game.drop_cube(Player::White);
        assert_eq!(game.score, [2, 0]);
        assert_eq!(
            game.message,
            "Computer dropped the cube. The game was worth 2 points."
        );
        assert_eq!(game.phase, Phase::GameOver(Player::White));
    }

    #[test]
    fn corrupt_autosaves_are_rejected_before_rendering() {
        let mut oversized_stack = Game::default();
        oversized_stack.position.points[0] = 127;
        assert!(Game::decode(&oversized_stack.encode()).is_none());

        let mut missing_checker = Game::default();
        missing_checker.position.points[23] -= 1;
        assert!(Game::decode(&missing_checker.encode()).is_none());

        let impossible_dice = Game {
            opening: false,
            dice: vec![6, 6, 5],
            ..Game::default()
        };
        assert!(Game::decode(&impossible_dice.encode()).is_none());

        let valid = Game {
            opening: false,
            ..Game::default()
        }
        .encode();
        let mut fields: Vec<_> = valid.split(';').map(str::to_owned).collect();
        fields[1] = "1".into();
        assert!(Game::decode(&fields.join(";")).is_none());
        fields[1] = "0,0".into();
        fields[5] = "3".into();
        assert!(Game::decode(&fields.join(";")).is_none());
        fields[5] = "1".into();
        fields[6] = "9".into();
        assert!(Game::decode(&fields.join(";")).is_none());
        fields[6] = "2".into();
        fields[14] = "offered-green".into();
        assert!(Game::decode(&fields.join(";")).is_none());
        fields[14] = "playing".into();
        fields[6] = "0".into();
        assert!(Game::decode(&fields.join(";")).is_none());
        fields[6] = "2".into();
        fields[14] = "offered-black".into();
        assert!(Game::decode(&fields.join(";")).is_none());
        fields[14] = "offered-white".into();
        assert!(Game::decode(&fields.join(";")).is_some());
        fields[3] = "1".into();
        fields[14] = "confirm-double".into();
        assert!(Game::decode(&fields.join(";")).is_none());

        let mut opening_black = Game::default()
            .encode()
            .split(';')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        opening_black[3] = "1".into();
        assert!(Game::decode(&opening_black.join(";")).is_none());

        let mut finished_playing = Game::default();
        finished_playing
            .position
            .points
            .iter_mut()
            .filter(|count| **count > 0)
            .for_each(|count| *count = 0);
        finished_playing.position.off[Player::White.index()] = CHECKERS;
        assert!(Game::decode(&finished_playing.encode()).is_none());

        let invalid_crawford_cube = Game {
            opening: false,
            cube: 2,
            cube_owner: Some(Player::Black),
            score: [4, 0],
            crawford_used: true,
            crawford_active: true,
            ..Game::default()
        };
        assert!(Game::decode(&invalid_crawford_cube.encode()).is_none());
    }

    #[test]
    fn blocked_playing_save_is_normalized_to_a_forced_pass() {
        let mut game = Game {
            opening: false,
            dice: vec![1],
            ..Game::default()
        };
        game.position.points[23] = -2;
        game.position.points[0] = 0;
        game.position.bar[Player::White.index()] = 1;
        game.position.off[Player::White.index()] = 1;
        assert!(legal_turns(&game.position, Player::White, &game.dice).is_empty());

        let restored = Game::decode(&game.encode()).expect("blocked save is safely repaired");
        assert_eq!(restored.turn, Player::Black);
        assert!(restored.dice.is_empty());
        assert_eq!(restored.selected, None);
        assert!(restored.history.is_empty());
        assert_eq!(restored.phase, Phase::Playing);
        assert_eq!(restored.message, "No legal play. Black to roll.");
    }

    #[test]
    fn numeric_renderer_caps_untrusted_values() {
        let mut canvas = Canvas::new(1072, BAND_HEIGHT);
        number(&mut canvas, 40, 2, u8::MAX, POINT_DIGITS, INK);
        assert!(canvas.pixels.contains(&INK));
    }

    #[test]
    fn restart_round_trips_pending_cube_offer() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        game.offer_double();
        assert_eq!(
            Game::decode(&game.encode()).map(|restored| restored.phase),
            Some(Phase::ConfirmDouble)
        );
        game.confirm_double();
        assert_eq!(
            Game::decode(&game.encode()).map(|restored| restored.phase),
            Some(Phase::Offered(Player::White))
        );
        game.phase = Phase::ConfirmDrop(Player::White);
        assert_eq!(
            Game::decode(&game.encode()).map(|restored| restored.phase),
            Some(Phase::ConfirmDrop(Player::White))
        );
    }

    #[test]
    fn restart_round_trips_terminal_game_and_match() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        game.position
            .points
            .iter_mut()
            .filter(|count| **count > 0)
            .for_each(|count| *count = 0);
        game.position.off = [CHECKERS, 0];
        game.finish_game(Player::White);
        assert_eq!(
            Game::decode(&game.encode()).map(|restored| restored.phase),
            Some(Phase::GameOver(Player::White))
        );
        let mut match_game = Game {
            match_to: 1,
            opening: false,
            ..Game::default()
        };
        match_game
            .position
            .points
            .iter_mut()
            .filter(|count| **count > 0)
            .for_each(|count| *count = 0);
        match_game.position.off = [CHECKERS, 0];
        match_game.finish_game(Player::White);
        assert_eq!(
            Game::decode(&match_game.encode()).map(|restored| restored.phase),
            Some(Phase::MatchOver(Player::White))
        );
    }

    #[test]
    fn the_checker_in_hand_is_solid_and_only_its_destinations_are_boxed() {
        let game = Game {
            position: position(&[(6, 1), (4, 1)], 0, 0),
            dice: vec![1],
            selected: Some(Selected::Point(6)),
            opening: false,
            ..Game::default()
        };
        assert_eq!(
            game.displayed_moves()
                .iter()
                .map(|play| play.to)
                .collect::<Vec<_>>(),
            vec![Some(5)]
        );
        let marks = point_marks(&game);
        assert_eq!(marks[6], Mark::Solid);
        assert_eq!(marks[5], Mark::Open);
        assert_eq!(marks[4], Mark::Plain, "another checker that could move");
        assert_eq!(
            band_mark(&game, 6),
            INK,
            "the checker in hand is not filled"
        );
        assert_eq!(band_mark(&game, 5), INK, "the destination has no frame");
        assert_eq!(band_mark(&game, 9), PAPER, "an unrelated point was marked");
    }

    #[test]
    fn every_point_is_a_tap_target_on_the_board_itself() {
        let mut game = Game {
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let board = board_rect(&test_screen(&game));
        for point in 0..POINTS {
            let rect = layout
                .rect_of_action(action_id(&point_name(point)))
                .unwrap_or_else(|| panic!("point {point} cannot be tapped"));
            assert!(
                rect.y >= board.y && rect.y + rect.height <= board.y + board.height,
                "point {point} is not on the board"
            );
            assert_eq!(
                layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                Some(action_id(&point_name(point)))
            );
        }
        // The numbered keys that used to stand in for the points are gone.
        for retired in [
            "points-next",
            "points-prev",
            "computer",
            "match",
            "how-to-play",
        ] {
            assert!(
                layout.rect_of_action(action_id(retired)).is_none(),
                "{retired}"
            );
        }
    }

    #[test]
    fn tapping_a_point_picks_up_its_checker_and_tapping_a_marked_point_moves_it() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        tap(&mut game, "roll");
        assert_eq!(game.dice, vec![2, 1]);
        tap(&mut game, "point-23");
        assert_eq!(game.selected, Some(Selected::Point(23)));
        assert_eq!(game.message, "Tap a marked point to move it there.");
        let cells = board_cells(&game);
        let open = cells
            .iter()
            .filter(|(_, label, _, _)| label.ends_with(", open"))
            .map(|(name, _, _, _)| name.clone())
            .collect::<Vec<_>>();
        assert_eq!(open, ["point-21", "point-22"], "{cells:?}");
        tap(&mut game, "point-21");
        assert_eq!(game.position.points[21], 1);
        assert_eq!(game.position.points[23], 1);
        assert_eq!(game.message, "1 left to play.");
    }

    #[test]
    fn a_wrong_tap_says_what_to_do_and_tapping_the_checker_again_puts_it_back() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        tap(&mut game, "roll");
        tap(&mut game, "point-0");
        assert_eq!(game.selected, None);
        assert_eq!(game.message, "Tap a point with one of your checkers on it.");
        tap(&mut game, "point-23");
        tap(&mut game, "point-15");
        assert_eq!(game.selected, Some(Selected::Point(23)));
        assert!(game
            .message
            .starts_with("That point is closed to this checker."));
        tap(&mut game, "point-12");
        assert_eq!(
            game.selected,
            Some(Selected::Point(12)),
            "another movable checker takes the place of the one in hand"
        );
        tap(&mut game, "point-12");
        assert_eq!(game.selected, None);
        assert!(game.message.starts_with("Checker put back."));
        assert_eq!(game.position, Position::initial());
    }

    #[test]
    fn an_empty_point_open_to_the_checker_in_hand_carries_the_legal_ring() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        game.tap_point(12);
        let cells = board_cells(&game);
        let ring = |point: usize| {
            cells
                .iter()
                .find(|(name, _, _, _)| *name == point_name(point))
                .and_then(|(_, _, glyph, _)| *glyph)
        };
        assert_eq!(ring(10), Some(Glyph::LegalPoint));
        assert_eq!(
            ring(11),
            Some(Glyph::BlackDisc),
            "a closed point keeps its checkers"
        );
        assert_eq!(ring(9), None, "two dice together are two moves, not one");
        assert_eq!(
            ring(12),
            Some(Glyph::WhiteDisc),
            "the stack keeps its checkers"
        );
        assert_eq!(ring(8), None, "a point out of reach stays empty");
    }

    #[test]
    fn a_stack_taller_than_the_runtime_draws_is_counted_beside_its_number() {
        let mut game = Game {
            position: position(&[(5, 7), (4, 8), (18, -15)], 0, 0),
            opening: false,
            ..Game::default()
        };
        let cells = board_cells(&game);
        assert!(cells
            .iter()
            .any(|(_, label, _, _)| label == "Point 6 white 7 checkers"));
        let plain = band_pixels(&Game::default(), 1072, false);
        let counted = band_pixels(&game, 1072, false);
        let column = |pixels: &[u8], slot: usize| {
            (0..usize::try_from(BAND_HEIGHT).expect("fits"))
                .flat_map(|y| (slot * 89..slot * 89 + 89).map(move |x| y * 1072 + x))
                .filter(|index| pixels[*index] == INK)
                .count()
        };
        // The six point sits seventh from the left of the bottom row.
        assert!(
            column(&counted, 6) > column(&plain, 6) * 2,
            "no counter drawn"
        );
        game.position = position(&[(5, 5), (4, 10), (18, -15)], 0, 0);
        assert_eq!(
            column(&band_pixels(&game, 1072, false), 6),
            column(&plain, 6),
            "five checkers need no counter"
        );
    }

    #[test]
    fn nothing_on_the_board_moves_for_the_player_while_the_computer_is_stuck() {
        // The computer plays the moment its turn comes, so its turn is only
        // ever on screen when its dice cannot be rolled.
        let mut game = Game {
            mode: Mode::Solo,
            turn: Player::Black,
            opening: false,
            dice: vec![1],
            ..Game::default()
        };
        let before = game.encode();
        for action in [
            "roll", "double", "bar", "off", "point-0", "point-23", "take", "drop",
        ] {
            assert_eq!(game_action(&mut game, action_id(action)), None, "{action}");
            assert_eq!(game.encode(), before, "{action} changed the game");
        }
        assert_eq!(game_action(&mut game, action_id(game::UNDO)), Some(()));
        assert_eq!(game.message, game::NOTHING_TO_UNDO);
        assert_eq!(game.encode(), before);
        let diagnostics =
            test_screen(&game).diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);

        game.phase = Phase::Offered(Player::Black);
        let offered = game.encode();
        for action in ["take", "drop", "confirm-drop"] {
            assert_eq!(game_action(&mut game, action_id(action)), None, "{action}");
            assert_eq!(game.encode(), offered, "{action} changed a cube offer");
        }
    }

    #[test]
    fn computer_does_not_play_white_after_an_invalid_opening_black_turn() {
        // The opening roll decides who starts, whatever the board said before
        // it. Here it falls to White, so the computer has nothing to play.
        let mut game = Game {
            mode: Mode::Solo,
            turn: Player::Black,
            dice_source: Dice::scripted([(6, 1)]),
            ..Game::default()
        };
        assert!(
            !game.play_computer_turn(),
            "an opening is never the computer's"
        );
        game.opening = true;
        game.roll();
        assert!(!game.play_computer_turn());
        assert_eq!(game.turn, Player::White);
        assert_eq!(game.position, Position::initial());
        assert!(!game.dice.is_empty());
    }

    #[test]
    fn undo_is_committed_when_a_pass_and_play_turn_ends() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
            dice_source: Dice::scripted([(2, 1), (3, 2)]),
            ..Game::default()
        };
        game.roll();
        game.select(Some(23));
        game.move_to(Some(22));
        assert_eq!(game.history.len(), 1);
        game.select(Some(22));
        game.move_to(Some(20));
        assert_eq!(game.turn, Player::Black);
        assert!(game.history.is_empty());
        game.roll();
        game_action(&mut game, action_id(game::UNDO));
        assert_eq!(game.turn, Player::Black);
        assert_eq!(game.message, game::NOTHING_TO_UNDO);
    }

    #[test]
    fn opening_cannot_double_and_match_length_restarts_everything() {
        let mut game = Game::default();
        game.offer_double();
        assert_eq!(game.phase, Phase::Playing);
        assert_eq!(game.message, "Nobody can double before the opening roll.");

        game.cube = 8;
        game.cube_owner = Some(Player::White);
        game.crawford_used = true;
        game.crawford_active = true;
        game.history.push(game.snapshot());
        // The length of the match is set where the match is, not beside the
        // controls that play it.
        game_action(&mut game, action_id("match-settings"));
        assert_eq!(game.view, View::Match);
        assert_eq!(game.match_to, 5, "opening the settings changed the match");
        game_action(&mut game, action_id("length-7"));
        assert_eq!(game.match_to, 7);
        assert_eq!(game.position, Position::initial());
        assert_eq!((game.cube, game.cube_owner), (1, None));
        assert!(!game.crawford_used && !game.crawford_active && game.history.is_empty());

        game_action(&mut game, action_id("length-1"));
        assert_eq!(game.match_to, 1);
        assert!(!game.crawford_active);
        game_action(&mut game, action_id("close-match"));
        assert_eq!(game.view, View::Board);
    }

    #[test]
    fn match_length_is_stated_not_offered_once_the_match_has_begun() {
        let mut game = Game {
            dice_source: Dice::scripted([(5, 3)]),
            mode: Mode::PassAndPlay,
            ..Game::default()
        };
        game.roll();
        game.view = View::Match;
        let screen = test_screen(&game);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(layout.rect_of_action(action_id("length-7")).is_none());
        assert!(layout.rect_of_action(action_id("mode-solo")).is_none());
        let text = drawn(&screen);
        assert!(text.contains("This match plays to 5 points."), "{text}");
        assert!(text.contains("Start a new game from the menu"), "{text}");
        assert_eq!(game_action(&mut game, action_id("length-7")), None);
        assert_eq!(game.match_to, 5);
    }

    #[test]
    fn cube_stops_at_sixty_four_without_overflowing_the_picture() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        for expected in [2, 4, 8, 16, 32, MAX_CUBE] {
            game.offer_double();
            game.confirm_double();
            let offered = game.turn;
            game.take(offered);
            assert_eq!(game.cube, expected);
            game.turn = game.cube_owner.expect("taken cube has an owner");
        }
        game.offer_double();
        assert_eq!(game.phase, Phase::Playing);
        assert_eq!(game.message, "The cube is already at its maximum of 64.");
        assert!(cube_line(&game).starts_with("64 "));
    }

    #[test]
    fn finished_games_score_cube_and_schedule_crawford_at_match_point() {
        let mut game = Game::default();
        game.position.off[Player::White.index()] = CHECKERS;
        game.position.off[Player::Black.index()] = 1;
        game.cube = 2;
        game.finish_game(Player::White);
        assert_eq!(game.score, [2, 0]);
        assert_eq!(game.phase, Phase::GameOver(Player::White));
        game.score = [4, 0];
        game.start_game();
        assert!(game.crawford_active);
        assert!(game.crawford_used);
    }

    /// A seeded run replays exactly, and two seeds do not agree. Without this
    /// the dice were a counter: every game dealt the same rolls in the same
    #[test]
    fn a_denied_autosave_is_announced_until_a_write_lands() {
        use kobo_sdk::{AppRunner, StoreError};

        let mut runner = AppRunner::new(Game::default());
        runner.store_result(StoreResult::Denied(StoreError::NoRoom));
        assert!(runner.app().save == SaveState::Refused);
        let drawn = format!("{:?}", test_screen(runner.app()));
        assert!(drawn.contains("Progress is not saved"), "{drawn}");
        runner.store_result(StoreResult::Saved {
            key: SAVE.to_owned(),
        });
        assert!(runner.app().save == SaveState::Kept);
        let drawn = format!("{:?}", test_screen(runner.app()));
        assert!(!drawn.contains("Progress is not saved"), "{drawn}");
    }

    /// order, which is not a game of backgammon.
    #[test]
    fn seeded_dice_replay_and_two_seeds_differ_while_staying_in_range() {
        let sequence = |seed: u64| {
            let mut game = Game {
                dice_source: Dice::Fixture(FixtureEntropy::new(seed)),
                ..Game::default()
            };
            (0..120)
                .map(|_| game.take_roll().expect("a seeded source always rolls"))
                .collect::<Vec<_>>()
        };
        let first = sequence(7);
        assert_eq!(first, sequence(7), "the same seed dealt different dice");
        assert_ne!(first, sequence(8), "two seeds dealt the same dice");
        assert!(first
            .iter()
            .all(|(a, b)| (1..=6).contains(a) && (1..=6).contains(b)));
        let mut faces = [0usize; 6];
        for (a, b) in &first {
            faces[usize::from(a - 1)] += 1;
            faces[usize::from(b - 1)] += 1;
        }
        assert!(
            faces.iter().all(|count| *count > 0),
            "a face never came up: {faces:?}"
        );
    }

    /// A source that cannot be opened says so and rolls nothing, rather than
    /// falling back to a sequence that looks like chance.
    #[test]
    fn dice_that_cannot_be_rolled_stop_the_turn_and_say_why() {
        let mut game = Game {
            dice_source: Dice::Unavailable("No dice: the source is missing.".into()),
            ..Game::default()
        };
        assert!(game.take_roll().is_none());
        game.roll();
        assert!(game.dice.is_empty(), "a broken source still produced dice");
        assert!(game.message.contains("No dice"), "{}", game.message);
    }

    #[test]
    fn consumer_board_fits_clara_bw_without_diagnostics() {
        let mut game = Game {
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("point-23")).is_some());
        assert!(layout.rect_of_action(action_id("computer")).is_none());
        let diagnostics = test_screen(&game).diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn the_board_is_the_runtimes_backgammon_board_numbered_by_two_strips() {
        let mut game = Game {
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let pictures = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::Picture(handle) => Some((handle, node.rect)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let board = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::BackgammonBoard))
            .expect("the runtime's board")
            .rect;
        assert_eq!(pictures.len(), 2);
        let (top, bottom) = (pictures[0], pictures[1]);
        assert_eq!(top.0, TOP_BAND);
        assert_eq!(bottom.0, BOTTOM_BAND);
        // One pixel to one pixel, across the same width as the board, so
        // every number sits beside its own point.
        for (_, rect) in [top, bottom] {
            assert_eq!((rect.x, rect.width), (board.x, board.width));
            assert_eq!(rect.height, i32::try_from(BAND_HEIGHT).expect("fits"));
        }
        assert!(top.1.y + top.1.height <= board.y);
        assert!(bottom.1.y >= board.y + board.height);
    }

    /// What a table can read from across the board, and what a reader picking
    /// the panel back up needs first: whose move it is, what the dice said,
    /// and where the cube stands.
    #[test]
    fn the_board_states_the_turn_the_dice_and_the_cube_at_every_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut game = Game {
                dice_source: Dice::scripted([(5, 3)]),
                mode: Mode::PassAndPlay,
                cube: 2,
                cube_owner: Some(Player::White),
                score: [3, 1],
                ..Game::default()
            };
            game.roll();
            game.played.push("Black 13/8 24/23".into());
            // The board picks how much status it can show, and the history is
            // paged, by measuring against the context they are built with. A
            // default context made both choices for the default size, so the
            // board was checked here at sizes it had never been fitted to.
            let runner = kobo_sdk::AppRunner::with_metrics(Game::default(), metrics);
            let context = runner.context();
            // The rules are measured against the panel they are paged for,
            // which the rules' own test covers at every size.
            for view in [View::Board, View::Match, View::History] {
                game.view = view;
                let screen = screen_for(&game, Some(test_bands()), &context);
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?} {view:?}: {:?}",
                    diagnostics.issues
                );
                let drawn = screen
                    .layout_with(&metrics, &Chrome::measuring(true))
                    .nodes
                    .iter()
                    .flat_map(|node| node.text_lines.clone())
                    .collect::<Vec<_>>()
                    .join(" ");
                match view {
                    View::Board => {
                        assert!(drawn.contains("to play"), "{text_scale:?}: {drawn}");
                        assert!(drawn.contains("5 and 3"), "{text_scale:?}: {drawn}");
                    }
                    View::Match => {
                        assert!(drawn.contains("Points to win"), "{drawn}");
                    }
                    View::History => {
                        assert!(drawn.contains("Black 13/8 24/23"), "{drawn}");
                    }
                    View::Help | View::NewMatch => {}
                }
            }
        }
    }

    /// The record is what each side did, in the notation written beside a real
    /// board, counted from the side that made the move.
    #[test]
    fn finished_turns_are_written_down_in_board_notation_and_survive_a_restart() {
        let mut game = Game {
            dice_source: Dice::scripted([(2, 1), (3, 2)]),
            mode: Mode::PassAndPlay,
            ..Game::default()
        };
        game.roll();
        game.select(Some(23));
        game.move_to(Some(22));
        game.select(Some(22));
        game.move_to(Some(20));
        assert_eq!(game.played, vec!["White 24/23 23/21".to_owned()]);
        assert_eq!(game.turn, Player::Black);

        game.roll();
        let from = game
            .next_moves()
            .first()
            .copied()
            .expect("Black has a legal play");
        game.select(from.from);
        game.move_to(from.to);
        let restored = Game::decode(&game.encode()).expect("the match reopens");
        assert_eq!(restored.played, game.played);
        assert!(restored
            .played
            .first()
            .is_some_and(|line| line.starts_with("White ")));

        // The record is bounded: an evening of backgammon is not a log file.
        let mut long = Game::default();
        for _ in 0..HISTORY * 2 {
            long.turn_plays.push(Move {
                from: Some(23),
                to: Some(22),
                die: 1,
            });
            long.record_turn();
        }
        assert_eq!(long.played.len(), HISTORY);
        assert_eq!(Game::decode(&long.encode()).unwrap().played, long.played);
    }

    /// A seeded run says so wherever the match is described, because a capture
    /// of fixed dice that read as chance would be a lie about the game.
    /// A match left half-played under the previous build opens where it was.
    #[test]
    fn a_game_saved_by_the_previous_version_is_carried_forward() {
        let mut game = Game {
            score: [2, 1],
            opening: false,
            dice_source: Dice::scripted([(3, 2)]),
            ..Game::default()
        };
        game.roll();
        // The shape the previous version wrote: a roll counter where the
        // record of play now lives.
        let previous = game
            .encode()
            .splitn(15, ';')
            .enumerate()
            .map(|(index, field)| if index == 12 { "42" } else { field })
            .collect::<Vec<_>>()
            .join(";");
        let carried = Game::decode_previous(&previous).expect("the old save is understood");
        assert_eq!(carried.score, [2, 1]);
        assert_eq!(carried.dice, game.dice);
        assert!(carried.played.is_empty());
        assert!(Game::decode_previous(&game.encode()).is_none());
    }

    #[test]
    fn a_seeded_run_names_its_seed_on_the_match_screen() {
        let game = Game {
            dice_source: Dice::Fixture(FixtureEntropy::new(11)),
            view: View::Match,
            ..Game::default()
        };
        let drawn = test_screen(&game)
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(drawn.contains("Fixture dice, seed 11"), "{drawn}");
    }

    #[test]
    fn board_rows_are_contiguous_and_run_in_opposite_directions() {
        let game = Game::default();
        assert_eq!(
            board_order(&game),
            [
                12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1,
                0
            ]
        );
        assert_eq!(point_slot(&game, 12), (0, 0));
        assert_eq!(point_slot(&game, 23), (0, 11));
        assert_eq!(point_slot(&game, 11), (1, 0));
        assert_eq!(point_slot(&game, 0), (1, 11));
    }

    #[test]
    fn pass_and_play_rotates_the_board_without_breaking_adjacency() {
        let game = Game {
            mode: Mode::PassAndPlay,
            turn: Player::Black,
            ..Game::default()
        };
        assert_eq!(
            board_order(&game),
            [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13,
                12
            ]
        );
        assert_eq!(point_slot(&game, 0), (0, 0));
        assert_eq!(point_slot(&game, 11), (0, 11));
        assert_eq!(point_slot(&game, 23), (1, 0));
        assert_eq!(point_slot(&game, 12), (1, 11));
    }

    #[test]
    fn the_strips_number_each_point_from_the_side_the_board_faces() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
            ..Game::default()
        };
        assert_eq!(shown_number(&game, 0), 1);
        assert_eq!(shown_number(&game, 23), 24);
        game.turn = Player::Black;
        assert_eq!(shown_number(&game, 0), 24);
        assert_eq!(shown_number(&game, 23), 1);
        assert!(board_cells(&game)
            .iter()
            .any(|(name, label, _, _)| name == "point-23" && label.starts_with("Point 1 ")));
        game.mode = Mode::Solo;
        assert_eq!(
            shown_number(&game, 23),
            24,
            "the computer's side never turns the board"
        );
    }

    #[test]
    fn the_computer_plays_its_turn_as_soon_as_the_players_turn_ends() {
        let mut game = Game {
            dice_source: Dice::scripted([(2, 1), (6, 5)]),
            ..Game::default()
        };
        tap(&mut game, "roll");
        assert_eq!(game.turn, Player::White);
        tap(&mut game, "point-23");
        tap(&mut game, "point-21");
        tap(&mut game, "point-21");
        assert_eq!(game.turn, Player::White, "a die is still to play");
        tap(&mut game, "point-20");
        // No tap for the computer: its roll and its moves came with the
        // player's last checker.
        assert_eq!(game.turn, Player::White);
        assert!(game.dice.is_empty());
        let last = game.last_turn.clone().expect("the computer's turn is kept");
        assert_eq!(last.player, Player::Black);
        assert_eq!(last.plays.len(), 2);
        assert!(
            game.message
                .starts_with("Computer rolled 6 and 5 and played "),
            "{}",
            game.message
        );
        assert_eq!(game.played.len(), 2);
        let screen = test_screen(&game);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(layout.rect_of_action(action_id("computer")).is_none());
        // Where its checkers came from is boxed and where they went is solid.
        let marks = point_marks(&game);
        for play in &last.plays {
            if let Some(to) = play.to {
                assert_eq!(marks[to], Mark::Solid, "{play:?}");
            }
        }
        assert!(marks.contains(&Mark::Open));
        // The marks go when the player rolls, and the board shows what can
        // move instead.
        game.dice_source = Dice::scripted([(3, 1)]);
        tap(&mut game, "roll");
        assert!(game.last_turn.is_none());
    }

    #[test]
    fn the_computers_move_is_written_in_the_numbers_printed_beside_the_points() {
        let mut game = Game {
            turn: Player::Black,
            opening: false,
            dice_source: Dice::scripted([(6, 5)]),
            ..Game::default()
        };
        assert!(game.play_computer_turn());
        let last = game.last_turn.clone().expect("a turn was played");
        let words = game
            .message
            .trim_end_matches('.')
            .rsplit("played ")
            .next()
            .expect("the moves")
            .to_owned();
        let printed = last
            .plays
            .iter()
            .map(|play| {
                let side = |point: Option<usize>, none: &str| {
                    point.map_or_else(|| none.to_owned(), |p| shown_number(&game, p).to_string())
                };
                format!("{}/{}", side(play.from, "bar"), side(play.to, "off"))
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(words, printed);
        assert_eq!(game.played.last(), Some(&format!("Computer {printed}")));
    }

    #[test]
    fn a_player_who_cannot_move_is_told_before_the_computer_replies() {
        let mut game = Game {
            position: position(
                &[
                    (5, 14),
                    (18, -2),
                    (19, -2),
                    (20, -2),
                    (21, -2),
                    (22, -2),
                    (23, -2),
                    (0, -3),
                ],
                1,
                0,
            ),
            opening: false,
            dice_source: Dice::scripted([(2, 1), (6, 5)]),
            ..Game::default()
        };
        game.apply_action(action_id("roll"));
        assert_eq!(game.turn, Player::White);
        assert!(
            game.message
                .starts_with("You rolled 2 and 1 and cannot move. Computer rolled 6 and 5"),
            "{}",
            game.message
        );
    }

    #[test]
    fn the_board_keeps_its_size_and_place_for_a_whole_game() {
        let mut game = Game {
            dice_source: Dice::Fixture(FixtureEntropy::new(7)),
            ..Game::default()
        };
        let first = board_rect(&test_screen(&game));
        let mut turns = 0;
        while game.phase == Phase::Playing && turns < 400 {
            turns += 1;
            if game.dice.is_empty() {
                game.apply_action(action_id("roll"));
            } else {
                let play = game.next_moves()[0];
                match play.from {
                    Some(point) => game.apply_action(action_id(&point_name(point))),
                    None => game.apply_action(action_id("bar")),
                };
                match play.to {
                    Some(point) => game.apply_action(action_id(&point_name(point))),
                    None => game.apply_action(action_id("off")),
                };
            }
            if game.phase == Phase::Playing {
                assert_eq!(
                    board_rect(&test_screen(&game)),
                    first,
                    "after {turns} taps: {}",
                    game.message
                );
            }
        }
        assert!(
            matches!(game.phase, Phase::GameOver(_)),
            "the game did not finish in {turns} taps"
        );
    }

    #[test]
    fn the_shared_controls_sit_where_every_game_keeps_them() {
        let mut game = Game::default();
        let screen = test_screen(&game);
        let bar = screen.top_bar.clone().expect("a top bar");
        assert_eq!(bar.title, "Backgammon");
        let labels = bar
            .actions
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, [game::RULES_LABEL, "More"]);
        let foot = screen.nav_bar.clone().expect("a bar at the foot");
        let verbs = foot
            .destinations
            .iter()
            .map(|action| action.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(verbs, [game::UNDO_LABEL, "Roll", "Double"]);

        tap(&mut game, game::RULES);
        assert_eq!(game.view, View::Help);
        assert!(drawn(&test_screen(&game)).contains("Tap the checker again to put it back"));
        tap(&mut game, "close-help");
        assert_eq!(game.view, View::Board);

        tap(&mut game, game::MORE);
        assert!(game.menu.is_open());
        let menu = test_screen(&game)
            .overlay
            .expect("the open menu")
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_ui::Node::Button { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            menu,
            [game::NEW_GAME_LABEL, "Match settings", "Turn history"]
        );
        game.apply_action(action_id("turn-history"));
        assert!(!game.menu.is_open());
        assert_eq!(game.view, View::History);
        game.apply_action(ActionId::BACK);
        assert_eq!(game.view, View::Board);

        tap(&mut game, game::UNDO);
        assert_eq!(game.message, game::NOTHING_TO_UNDO);
    }

    #[test]
    fn undo_takes_back_one_move_of_the_turn_in_progress() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
            dice_source: Dice::scripted([(2, 1)]),
            ..Game::default()
        };
        game.roll();
        game.tap_point(23);
        game.tap_point(21);
        assert_eq!(game.turn_plays.len(), 1);
        tap(&mut game, game::UNDO);
        assert_eq!(game.position, Position::initial());
        assert_eq!(game.dice, vec![2, 1]);
        assert!(
            game.turn_plays.is_empty(),
            "the undone move would be recorded"
        );
        assert_eq!(game.message, "Move taken back.");
        tap(&mut game, game::UNDO);
        assert_eq!(game.message, game::NOTHING_TO_UNDO);
    }

    #[test]
    fn between_games_the_next_step_and_the_shared_controls_are_on_screen() {
        let mut game = Game {
            opening: false,
            ..Game::default()
        };
        game.position
            .points
            .iter_mut()
            .filter(|count| **count > 0)
            .for_each(|count| *count = 0);
        game.position.off = [CHECKERS, 0];
        game.finish_game(Player::White);
        let screen = test_screen(&game);
        let text = drawn(&screen);
        assert!(text.contains("You won this game."), "{text}");
        assert!(text.contains("The game was worth 3 points."), "{text}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        for name in ["next-game", game::UNDO, game::RULES, game::MORE] {
            assert!(layout.rect_of_action(action_id(name)).is_some(), "{name}");
        }
        assert!(layout.rect_of_action(action_id("new-match")).is_none());
        tap(&mut game, "next-game");
        assert_eq!(game.phase, Phase::Playing);
        assert_eq!(game.score, [3, 0]);

        game.score = [5, 0];
        game.phase = Phase::MatchOver(Player::White);
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(layout.rect_of_action(action_id(game::NEW_GAME)).is_some());
        tap(&mut game, game::NEW_GAME);
        assert_eq!(game.view, View::Board, "a finished match needs no question");
        assert_eq!(game.score, [0, 0]);
    }
}

#[cfg(test)]
mod new_match_tests;

#[cfg(test)]
mod help_tests;

#[cfg(test)]
mod match_history_tests;

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
    fn at_every_text_size_the_status_keeps_whole_rows_rather_than_losing_the_score() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..kobo_sdk::CLARA_BW_METRICS
            };
            let mut game = Game {
                dice_source: Dice::scripted([(4, 6)]),
                ..Game::default()
            };
            // The computer opens and leaves a two line note under the board.
            game.apply_action(action_id("roll"));
            let runner = kobo_sdk::AppRunner::with_metrics(game, metrics);
            let bands = Bands {
                top: TilePicture::new(TOP_BAND, 1072, BAND_HEIGHT),
                bottom: TilePicture::new(BOTTOM_BAND, 1072, BAND_HEIGHT),
            };
            let screen = screen_for(runner.app(), Some(bands), &runner.context());
            fits(&screen, metrics);
            let text = screen
                .layout_with(&metrics, &kobo_sdk::Chrome::measuring(true))
                .nodes
                .iter()
                .flat_map(|node| node.text_lines.clone())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                text.contains("Computer opened with"),
                "{text_scale:?}: {text}"
            );
            if text.contains("Cube") {
                assert!(
                    text.contains("first to 5"),
                    "{text_scale:?} lost the score: {text}"
                );
            }
        }
    }

    #[test]
    fn rolled_board_picture_leaves_room_for_all_controls() {
        for metrics in panels() {
            let mut game = Game {
                dice_source: Dice::scripted([(5, 3)]),
                ..Game::default()
            };
            game.roll();
            let runner = kobo_sdk::AppRunner::with_metrics(game, metrics);
            let width = u32::try_from(metrics.width).expect("panel width");
            let bands = Bands {
                top: TilePicture::new(TOP_BAND, width, BAND_HEIGHT),
                bottom: TilePicture::new(BOTTOM_BAND, width, BAND_HEIGHT),
            };
            let screen = screen_for(runner.app(), Some(bands), &runner.context());
            fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &kobo_sdk::Chrome::measuring(true));
            for name in ["roll", "double", game::UNDO, "bar", "off", "point-0"] {
                assert!(layout.rect_of_action(action_id(name)).is_some());
            }
        }
    }
}
