//! Complete, touch-first backgammon rules for a portrait Kobo panel.
use kobo_sdk::{
    action_id, ActionId, Context, KoboApp, PictureHandle, Screen, ScreenBuilder, StoreResult,
    TilePicture,
};
use std::process::ExitCode;

const POINTS: usize = 24;
const CHECKERS: u8 = 15;
const MAX_CUBE: u8 = 64;
const POINTS_PER_PAGE: usize = 6;
const SAVE: &str = "backgammon-autosave-v3";
const BOARD_PICTURE: PictureHandle = PictureHandle(1);
const BOARD_WIDTH: u32 = 960;
const BOARD_HEIGHT: u32 = 580;

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

impl Mode {
    const fn next(self) -> Self {
        match self {
            Self::Solo => Self::PassAndPlay,
            Self::PassAndPlay => Self::Solo,
        }
    }
    const fn label(self) -> &'static str {
        match self {
            Self::Solo => "Solo",
            Self::PassAndPlay => "Pass and play",
        }
    }
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

struct Game {
    position: Position,
    turn: Player,
    dice: Vec<u8>,
    selected: Option<Selected>,
    point_page: usize,
    cube: u8,
    cube_owner: Option<Player>,
    score: [u8; 2],
    match_to: u8,
    crawford_used: bool,
    crawford_active: bool,
    opening: bool,
    mode: Mode,
    rolls: usize,
    phase: Phase,
    history: Vec<Snapshot>,
    message: String,
    initial_load: InitialLoad,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            position: Position::initial(),
            turn: Player::White,
            dice: Vec::new(),
            selected: None,
            point_page: 0,
            cube: 1,
            cube_owner: None,
            score: [0; 2],
            match_to: 5,
            crawford_used: false,
            crawford_active: false,
            opening: true,
            mode: Mode::Solo,
            rolls: 0,
            phase: Phase::Playing,
            history: Vec::new(),
            message: "Tap Roll to begin.".into(),
            initial_load: InitialLoad::Pending,
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
        self.point_page = 0;
        self.phase = Phase::Playing;
        self.message = "Move restored.".into();
    }

    fn take_roll(&mut self) -> (u8, u8) {
        let first = u8::try_from(self.rolls % 6 + 1).expect("die fits u8");
        let second = u8::try_from(self.rolls / 6 % 6 + 1).expect("die fits u8");
        self.rolls += 1;
        (first, second)
    }

    fn roll(&mut self) {
        if self.phase != Phase::Playing || !self.dice.is_empty() {
            return;
        }
        self.history.clear();
        self.point_page = 0;
        let opening = self.opening;
        let (first, second) = loop {
            let roll = self.take_roll();
            if !self.opening || roll.0 != roll.1 {
                break roll;
            }
        };
        if opening {
            self.turn = if first > second {
                Player::White
            } else {
                Player::Black
            };
            self.opening = false;
        }
        self.dice = if first == second {
            vec![first; 4]
        } else {
            vec![first, second]
        };
        if legal_turns(&self.position, self.turn, &self.dice).is_empty() {
            self.end_turn();
            self.message = format!("No legal play. {} to roll.", self.turn.name());
        } else {
            self.message = format!(
                "{} {} {first} and {second}. {}",
                self.turn.name(),
                if opening { "opened with" } else { "rolled" },
                if self.next_moves().len() == 1 {
                    "Only one legal play — tap its checker."
                } else {
                    "Tap a checker."
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
        self.dice.clear();
        self.selected = None;
        self.point_page = 0;
        self.history.clear();
        self.turn = self.turn.other();
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
            self.point_page = 0;
            self.message = "Checker selected. Tap a legal destination.".into();
        } else {
            self.message = "That checker has no legal point for this roll.".into();
        }
    }

    fn move_to(&mut self, to: Option<usize>) {
        let Some(selected) = self.selected else {
            self.message = "Tap a checker first.".into();
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
            self.message = "Choose a legal destination.".into();
            return;
        };
        self.history.push(self.snapshot());
        self.position.apply(self.turn, play);
        let index = self
            .dice
            .iter()
            .position(|die| *die == play.die)
            .expect("legal move has a remaining die");
        self.dice.remove(index);
        self.selected = None;
        self.point_page = 0;
        if self.position.won(self.turn) {
            self.finish_game(self.turn);
        } else if self.dice.is_empty()
            || legal_turns(&self.position, self.turn, &self.dice).is_empty()
        {
            self.end_turn();
            self.message = format!("Move recorded. {} to roll.", self.turn.name());
        } else {
            self.message = "Use the remaining die.".into();
        }
    }

    fn offer_double(&mut self) {
        if self.phase != Phase::Playing || !self.dice.is_empty() {
            self.message = "Finish the roll before offering the cube.".into();
        } else if self.opening {
            self.message = "The opening roll must decide who starts.".into();
        } else if self.crawford_active {
            self.message = "The Crawford game has no cube.".into();
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
        self.after_score(offered, "wins by drop");
    }

    fn finish_game(&mut self, winner: Player) {
        let earned = self.cube.saturating_mul(self.position.points_worth(winner));
        self.score[winner.index()] = self.score[winner.index()].saturating_add(earned);
        self.after_score(winner, "wins the game");
    }

    fn after_score(&mut self, winner: Player, result: &str) {
        self.dice.clear();
        self.selected = None;
        self.point_page = 0;
        self.history.clear();
        self.message = format!("{} {result}.", winner.name());
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
        self.point_page = 0;
        self.cube = 1;
        self.cube_owner = None;
        self.phase = Phase::Playing;
        self.history.clear();
        self.message = if self.crawford_active {
            "Crawford game. The cube is inactive.".into()
        } else {
            "Tap Roll to begin.".into()
        };
    }

    fn start_match(&mut self) {
        self.score = [0; 2];
        self.crawford_used = false;
        self.start_game();
    }

    fn set_match_to(&mut self, match_to: u8) {
        self.match_to = match_to;
        self.start_match();
        self.message = format!("Match to {match_to}. Tap Roll to begin.");
    }

    fn computer_move(&mut self) {
        if self.mode != Mode::Solo || self.turn != Player::Black || self.phase != Phase::Playing {
            return;
        }
        if self.dice.is_empty() {
            self.roll();
        }
        if self.turn != Player::Black || self.phase != Phase::Playing {
            return;
        }
        while !self.dice.is_empty() && self.phase == Phase::Playing {
            let mut choices = self.next_moves();
            choices.sort_by_key(|play| std::cmp::Reverse(self.score_after(*play)));
            if let Some(play) = choices.first().copied() {
                self.selected = Some(match play.from {
                    Some(point) => Selected::Point(point),
                    None => Selected::Bar,
                });
                self.move_to(play.to);
            } else {
                break;
            }
        }
        if self.phase == Phase::Playing {
            self.message = "Computer move recorded. White to roll.".into();
        }
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
            self.rolls,
            u8::from(self.opening),
            self.phase.encode()
        )
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
            point_page: 0,
            cube: fields[5].parse().ok()?,
            cube_owner: saved_owner(fields[6]).ok()?,
            score: saved_pair(fields[7])?,
            match_to: fields[8].parse().ok()?,
            crawford_used: saved_flag(fields[9])?,
            crawford_active: saved_flag(fields[10])?,
            mode: saved_mode(fields[11])?,
            rolls: fields[12].parse().ok()?,
            opening: saved_flag(fields[13])?,
            phase,
            history: Vec::new(),
            message: saved_message(phase),
            initial_load: InitialLoad::Pending,
        };
        if !saved_game_is_safe(&game) {
            return None;
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

fn board_order(game: &Game) -> Vec<usize> {
    let mut order = (12..POINTS).chain((0..12).rev()).collect::<Vec<_>>();
    if game.mode == Mode::PassAndPlay && game.turn == Player::Black {
        order.reverse();
        order
    } else {
        order
    }
}

#[cfg(test)]
fn point_slot(game: &Game, point: usize) -> (usize, usize) {
    let slot = board_order(game)
        .iter()
        .position(|shown| *shown == point)
        .expect("all points are shown");
    (slot / 12, slot % 12)
}

fn point_controls(game: &Game) -> Vec<(String, String)> {
    if game.dice.is_empty() {
        return Vec::new();
    }
    let mut points = displayed_points(game);
    let pages = points.len().div_ceil(POINTS_PER_PAGE);
    let page = game.point_page.min(pages.saturating_sub(1));
    let start = page * POINTS_PER_PAGE;
    points = points
        .drain(start..points.len().min(start + POINTS_PER_PAGE))
        .collect();
    let mut controls = Vec::new();
    if page > 0 {
        controls.push(("points-prev".into(), "Prev".into()));
    }
    controls.extend(
        points
            .into_iter()
            .map(|point| (point_name(point), (point + 1).to_string())),
    );
    if page + 1 < pages {
        controls.push(("points-next".into(), "Next".into()));
    }
    controls
}

fn displayed_points(game: &Game) -> Vec<usize> {
    let mut points = Vec::new();
    for play in game.displayed_moves() {
        let point = if game.selected.is_some() {
            play.to
        } else {
            play.from
        };
        if let Some(point) = point {
            if !points.contains(&point) {
                points.push(point);
            }
        }
    }
    let order = board_order(game);
    points.sort_unstable_by_key(|point| {
        order
            .iter()
            .position(|shown| shown == point)
            .expect("shown point")
    });
    points
}

fn put_pixel(pixels: &mut [u8], x: i32, y: i32, tone: u8) {
    if (0..i32::try_from(BOARD_WIDTH).expect("board width fits i32")).contains(&x)
        && (0..i32::try_from(BOARD_HEIGHT).expect("board height fits i32")).contains(&y)
    {
        pixels[usize::try_from(y).expect("checked y")
            * usize::try_from(BOARD_WIDTH).expect("width")
            + usize::try_from(x).expect("checked x")] = tone;
    }
}

fn fill_rect(pixels: &mut [u8], x: i32, y: i32, width: i32, height: i32, tone: u8) {
    for row in y..y + height {
        for column in x..x + width {
            put_pixel(pixels, column, row, tone);
        }
    }
}

fn stroke_rect(pixels: &mut [u8], x: i32, y: i32, width: i32, height: i32, tone: u8) {
    fill_rect(pixels, x, y, width, 2, tone);
    fill_rect(pixels, x, y + height - 2, width, 2, tone);
    fill_rect(pixels, x, y, 2, height, tone);
    fill_rect(pixels, x + width - 2, y, 2, height, tone);
}

fn fill_circle(pixels: &mut [u8], centre_x: i32, centre_y: i32, radius: i32, tone: u8) {
    for y in -radius..=radius {
        for x in -radius..=radius {
            if x * x + y * y <= radius * radius {
                put_pixel(pixels, centre_x + x, centre_y + y, tone);
            }
        }
    }
}

fn checker(pixels: &mut [u8], x: i32, y: i32, white: bool) {
    let fill = if white { 244 } else { 38 };
    fill_circle(pixels, x, y, 25, fill);
    for vertical in -25..=25 {
        for horizontal in -25..=25 {
            let distance = horizontal * horizontal + vertical * vertical;
            if (23 * 23..=25 * 25).contains(&distance) {
                put_pixel(pixels, x + horizontal, y + vertical, 28);
            }
        }
    }
    if !white {
        fill_circle(pixels, x - 7, y - 7, 5, 112);
    }
}

fn triangle(pixels: &mut [u8], x: i32, width: i32, top: bool, dark: bool) {
    let base: i32 = if top { 28 } else { 552 };
    let apex_y: i32 = if top { 270 } else { 310 };
    let tone = if dark { 102 } else { 182 };
    let span = (base - apex_y).abs();
    for step in 0..=span {
        let y = if top { base + step } else { base - step };
        let half = (width / 2) * (span - step) / span;
        fill_rect(pixels, x + width / 2 - half, y, half * 2 + 1, 1, tone);
    }
}

fn seven_segment_digit(pixels: &mut [u8], x: i32, y: i32, digit: u8, tone: u8) {
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
    let segments = SEGMENTS[usize::from(digit)];
    let strokes = [
        (x + 2, y, 8, 2),
        (x + 10, y + 2, 2, 8),
        (x + 10, y + 12, 2, 8),
        (x + 2, y + 20, 8, 2),
        (x, y + 12, 2, 8),
        (x, y + 2, 2, 8),
        (x + 2, y + 10, 8, 2),
    ];
    for (on, (left, top, width, height)) in segments.into_iter().zip(strokes) {
        if on {
            fill_rect(pixels, left, top, width, height, tone);
        }
    }
}

fn number(pixels: &mut [u8], x: i32, y: i32, value: u8, tone: u8) {
    let value = value.min(99);
    if value >= 10 {
        seven_segment_digit(pixels, x, y, value / 10, tone);
        seven_segment_digit(pixels, x + 15, y, value % 10, tone);
    } else {
        seven_segment_digit(pixels, x, y, value, tone);
    }
}

fn pip(pixels: &mut [u8], x: i32, y: i32) {
    fill_circle(pixels, x, y, 5, 32);
}

fn die(pixels: &mut [u8], x: i32, y: i32, value: u8) {
    fill_rect(pixels, x, y, 42, 42, 240);
    stroke_rect(pixels, x, y, 42, 42, 40);
    let positions = [(11, 11), (31, 11), (21, 21), (11, 31), (31, 31)];
    let dots: &[usize] = match value {
        1 => &[2],
        2 => &[0, 4],
        3 => &[0, 2, 4],
        4 => &[0, 1, 3, 4],
        5 => &[0, 1, 2, 3, 4],
        6 => &[0, 1, 3, 4, 0, 1],
        _ => &[],
    };
    if value == 6 {
        pip(pixels, x + 11, y + 9);
        pip(pixels, x + 31, y + 9);
        pip(pixels, x + 11, y + 21);
        pip(pixels, x + 31, y + 21);
        pip(pixels, x + 11, y + 33);
        pip(pixels, x + 31, y + 33);
    } else {
        for index in dots {
            let (dot_x, dot_y) = positions[*index];
            pip(pixels, x + dot_x, y + dot_y);
        }
    }
}

fn point_x(column: usize) -> i32 {
    const POINT_WIDTH: i32 = 71;
    if column < 6 {
        18 + i32::try_from(column).expect("column") * POINT_WIDTH
    } else {
        518 + i32::try_from(column - 6).expect("column") * POINT_WIDTH
    }
}

fn draw_points(pixels: &mut [u8], game: &Game, order: &[usize]) {
    const POINT_WIDTH: i32 = 71;
    for (slot, point) in order.iter().enumerate() {
        let row = slot / 12;
        let column = slot % 12;
        let x = point_x(column);
        triangle(pixels, x, POINT_WIDTH, row == 0, (column + row) % 2 == 0);
        let count = game.position.points[*point].unsigned_abs();
        if count > 0 {
            let white = game.position.points[*point] > 0;
            let shown = count.min(5);
            for stack in 0..shown {
                let y = if row == 0 {
                    55 + i32::from(stack) * 46
                } else {
                    525 - i32::from(stack) * 46
                };
                checker(pixels, x + POINT_WIDTH / 2, y, white);
            }
            if count > shown {
                number(
                    pixels,
                    x + POINT_WIDTH / 2 - 7,
                    if row == 0 { 214 } else { 344 },
                    count,
                    if white { 32 } else { 244 },
                );
            }
        }
        number(
            pixels,
            x + POINT_WIDTH / 2 - if *point + 1 >= 10 { 15 } else { 7 },
            if row == 0 { 278 } else { 280 },
            u8::try_from(*point + 1).expect("point number fits"),
            44,
        );
    }
}

fn draw_move_markers(pixels: &mut [u8], game: &Game, order: &[usize]) {
    let moves = game.displayed_moves();
    for point in 0..POINTS {
        let marked = if game.selected.is_some() {
            moves.iter().any(|play| play.to == Some(point))
        } else {
            moves.iter().any(|play| play.from == Some(point))
        };
        if marked {
            let slot = order
                .iter()
                .position(|shown| *shown == point)
                .expect("each point shown");
            let row = slot / 12;
            let column = slot % 12;
            let x = point_x(column);
            let y = if row == 0 { 246 } else { 334 };
            fill_circle(pixels, x + 71 / 2, y, 10, 248);
            fill_circle(pixels, x + 71 / 2, y, 7, 40);
            fill_circle(pixels, x + 71 / 2, y, 4, 248);
        }
    }
}

fn draw_centre(pixels: &mut [u8], game: &Game) {
    for (player, y) in [(Player::White, 112), (Player::Black, 428)] {
        let count = game.position.bar[player.index()];
        if count > 0 {
            checker(pixels, 480, y, player == Player::White);
            number(
                pixels,
                473,
                y - 11,
                count,
                if player == Player::White { 32 } else { 244 },
            );
        }
    }
    fill_rect(pixels, 448, 274, 64, 32, 232);
    stroke_rect(pixels, 448, 274, 64, 32, 40);
    number(pixels, 465, 279, game.cube, 32);

    for (index, value) in game.dice.iter().take(4).enumerate() {
        let x = if index % 2 == 0 { 438 } else { 480 };
        let y = 168 + i32::try_from(index / 2).expect("index") * 48;
        die(pixels, x, y, *value);
    }
}

fn board_pixels(game: &Game) -> Vec<u8> {
    let mut pixels = vec![
        248;
        usize::try_from(BOARD_WIDTH).expect("width")
            * usize::try_from(BOARD_HEIGHT).expect("height")
    ];
    fill_rect(&mut pixels, 12, 12, 936, 556, 226);
    stroke_rect(&mut pixels, 12, 12, 936, 556, 30);
    fill_rect(&mut pixels, 444, 14, 72, 552, 148);
    stroke_rect(&mut pixels, 444, 14, 72, 552, 40);
    let order = board_order(game);
    draw_points(&mut pixels, game, &order);
    draw_move_markers(&mut pixels, game, &order);
    draw_centre(&mut pixels, game);
    pixels
}

fn screen(game: &Game, picture: Option<TilePicture>) -> Screen {
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
        Phase::GameOver(_) => ScreenBuilder::new("backgammon-game-over")
            .top_bar("Backgammon")
            .secondary(&game.message)
            .grid(
                2,
                false,
                [("next-game", "Next game"), ("new-match", "New match")],
            )
            .build(),
        Phase::MatchOver(winner) => ScreenBuilder::new("backgammon-match-over")
            .top_bar("Backgammon")
            .secondary(format!(
                "{} wins the match {}–{}.",
                winner.name(),
                game.score[0],
                game.score[1]
            ))
            .primary_button("new-match", "New match")
            .build(),
        Phase::Playing => playing_screen(game, picture),
    }
}

fn playing_screen(game: &Game, picture: Option<TilePicture>) -> Screen {
    let dice = if game.dice.is_empty() {
        "—".into()
    } else {
        game.dice
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let mut screen = ScreenBuilder::new("backgammon")
        .top_bar("Backgammon")
        .secondary(format!(
            "{} to play · {}–{} to {} · cube {} · {dice}",
            game.turn.name(),
            game.score[0],
            game.score[1],
            game.match_to,
            game.cube
        ))
        .secondary(if game.mode == Mode::PassAndPlay {
            format!("Pass the reader. {} is nearest.", game.turn.name())
        } else {
            "Solo: you are White; Computer is Black.".into()
        })
        .secondary(&game.message);
    if let Some(picture) = picture {
        screen = screen.unframed_picture(picture, 52);
    }
    if game.mode == Mode::Solo && game.turn == Player::Black {
        return screen
            .secondary("Computer's turn. Review the board, then play its move.")
            .bottom_action("computer", "Computer move")
            .build();
    }
    let bar_active = game.next_moves().iter().any(|play| play.from.is_none());
    let bar = checker_counter("Bar", game.position.bar);
    let bar = if bar_active {
        format!("○ {bar}")
    } else {
        bar
    };
    screen = screen.grid(
        2,
        false,
        [
            ("bar", bar),
            ("off", checker_counter("Off", game.position.off)),
        ],
    );
    screen
        .grid(8, true, point_controls(game))
        .chips([
            ("mode", game.mode.label().to_owned(), false),
            ("match", format!("To {}", game.match_to), false),
        ])
        .action_bar([("roll", "Roll"), ("double", "Double"), ("undo", "Undo")])
        .build()
}

fn checker_counter(label: &str, count: [u8; 2]) -> String {
    match count {
        [0, 0] => format!("{label} —"),
        [white, 0] => format!("{label} · White {white}"),
        [0, black] => format!("{label} · Black {black}"),
        [white, black] => format!("{label} · White {white} · Black {black}"),
    }
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SAVE);
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if let Some(persisted) = self.apply_action(action) {
            if persisted {
                context.store().save(SAVE, self.encode());
            }
            self.show(context);
        }
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == SAVE {
                let normalized = value
                    .as_deref()
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .is_some_and(|encoded| {
                        Self::decode(encoded).is_some_and(|saved| saved.encode() != encoded)
                    });
                if self.apply_initial_load(value) {
                    if normalized {
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
        let picture = if self.phase == Phase::Playing {
            context.put_picture(BOARD_PICTURE, BOARD_WIDTH, BOARD_HEIGHT, board_pixels(self))
        } else {
            None
        };
        context.set_screen(screen(self, picture));
    }
}

fn game_action(game: &mut Game, action: ActionId) -> Option<()> {
    if game.mode == Mode::Solo
        && game.turn == Player::Black
        && matches!(
            game.phase,
            Phase::Playing | Phase::ConfirmDouble | Phase::Offered(_) | Phase::ConfirmDrop(_)
        )
    {
        if game.phase == Phase::Playing && action == action_id("computer") {
            game.computer_move();
            return Some(());
        }
        return None;
    }
    match game.phase {
        Phase::ConfirmDouble if action == action_id("confirm-double") => {
            game.confirm_double();
            if game.mode == Mode::Solo && game.turn == Player::White {
                game.take(Player::White);
                game.message = format!("Computer takes. {} to roll.", game.turn.name());
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
        Phase::GameOver(_) | Phase::MatchOver(_) if action == action_id("new-match") => {
            game.start_match();
        }
        Phase::Playing if action == action_id("roll") => game.roll(),
        Phase::Playing if action == action_id("double") => game.offer_double(),
        Phase::Playing if action == action_id("undo") => {
            if let Some(snapshot) = game.history.pop() {
                game.restore(snapshot);
            } else {
                game.message = "No move to undo.".into();
            }
        }
        Phase::Playing if action == action_id("mode") && game.dice.is_empty() => {
            game.mode = game.mode.next();
            game.message = format!("{} selected.", game.mode.label());
        }
        Phase::Playing
            if action == action_id("match") && game.score == [0; 2] && game.dice.is_empty() =>
        {
            let match_to = match game.match_to {
                1 => 3,
                3 => 5,
                5 => 7,
                _ => 1,
            };
            game.set_match_to(match_to);
        }
        Phase::Playing if action == action_id("points-prev") => {
            game.point_page = game.point_page.saturating_sub(1);
        }
        Phase::Playing if action == action_id("points-next") => {
            let pages = displayed_points(game).len().div_ceil(POINTS_PER_PAGE);
            game.point_page = (game.point_page + 1).min(pages.saturating_sub(1));
        }
        Phase::Playing if action == action_id("bar") => game.select(None),
        Phase::Playing if action == action_id("off") => game.move_to(None),
        Phase::Playing => {
            let point = (0..POINTS).find(|point| action == action_id(&point_name(*point)))?;
            if game.selected.is_some() {
                game.move_to(Some(point));
            } else {
                game.select(Some(point));
            }
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
        screen(
            game,
            Some(TilePicture::new(BOARD_PICTURE, BOARD_WIDTH, BOARD_HEIGHT)),
        )
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
            ..Game::default()
        };
        game.roll();
        assert!(game.dice.is_empty());
        assert_eq!(game.turn, Player::Black);
    }

    #[test]
    fn opening_roll_rerolls_ties_and_awards_first_turn_to_higher_die() {
        let mut game = Game::default();
        game.roll();
        assert!(!game.opening);
        assert_eq!(game.turn, Player::White);
        assert_eq!(game.dice, vec![2, 1]);
        assert!(game.message.starts_with("White opened with 2 and 1"));
    }

    #[test]
    fn turn_entry_requires_all_usable_dice_before_switching_players() {
        let mut game = Game::default();
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
        assert_eq!(game.message, "The Crawford game has no cube.");
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
        assert_eq!(game.message, "Computer takes. White to roll.");
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
            rolls: 7,
            initial_load: InitialLoad::Settled,
            ..Game::default()
        };
        assert!(!game.apply_initial_load(Some(saved)));
        assert_eq!(game.rolls, 7);
    }

    #[test]
    fn a_no_op_action_does_not_discard_the_pending_initial_load() {
        let mut game = Game::default();
        assert_eq!(
            game.apply_action(action_id("computer")),
            None,
            "Computer move is not available on White's turn"
        );
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
        assert_eq!(game.message, "White wins by drop.");
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
        let mut pixels =
            vec![255; usize::try_from(BOARD_WIDTH * BOARD_HEIGHT).expect("board pixel count")];
        number(&mut pixels, 2, 2, u8::MAX, 32);
        assert!(pixels.contains(&32));
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
    fn selected_source_alone_supplies_destinations_and_markers() {
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
        let controls = point_controls(&game);
        assert_eq!(controls, vec![(point_name(5), "6".into())]);
        let pixels = board_pixels(&game);
        assert_eq!(
            pixels[334 * usize::try_from(BOARD_WIDTH).expect("width") + 553],
            248
        );
        assert_ne!(
            pixels[334 * usize::try_from(BOARD_WIDTH).expect("width") + 695],
            248
        );
    }

    #[test]
    fn point_controls_paginate_fifteen_legal_sources_without_hiding_actions() {
        let mut game = Game {
            position: position(
                &(6..=20)
                    .map(|point| (point, 1))
                    .collect::<Vec<(usize, i8)>>(),
                0,
                0,
            ),
            dice: vec![1],
            opening: false,
            ..Game::default()
        };
        let all_points = displayed_points(&game);
        assert_eq!(all_points.len(), 15);

        let mut reachable = Vec::new();
        loop {
            let controls = point_controls(&game);
            assert!(controls.len() <= 8);
            let layout =
                test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
            for (name, _) in &controls {
                assert!(
                    layout.rect_of_action(action_id(name)).is_some(),
                    "{name} must be reachable"
                );
                if name.starts_with("point-") {
                    reachable.push(name.clone());
                }
            }
            assert!(layout.rect_of_action(action_id("roll")).is_some());
            assert!(layout.rect_of_action(action_id("undo")).is_some());
            if !controls.iter().any(|(name, _)| name == "points-next") {
                break;
            }
            game_action(&mut game, action_id("points-next"));
        }
        reachable.sort_unstable();
        reachable.dedup();
        assert_eq!(reachable.len(), 15);
        assert_eq!(game.point_page, 2);
        let diagnostics =
            test_screen(&game).diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn solo_black_turn_rejects_direct_human_gameplay_actions() {
        let mut game = Game {
            mode: Mode::Solo,
            turn: Player::Black,
            opening: false,
            dice: vec![1],
            ..Game::default()
        };
        let before = game.encode();
        for action in [
            "roll",
            "double",
            "undo",
            "bar",
            "off",
            "point-0",
            "point-23",
            "points-next",
            "take",
            "drop",
        ] {
            assert_eq!(game_action(&mut game, action_id(action)), None, "{action}");
            assert_eq!(game.encode(), before, "{action} changed the game");
        }
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(layout.rect_of_action(action_id("computer")).is_some());
        for action in ["roll", "double", "undo", "bar", "off", "point-0"] {
            assert!(
                layout.rect_of_action(action_id(action)).is_none(),
                "{action}"
            );
        }
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
        let mut game = Game {
            mode: Mode::Solo,
            turn: Player::Black,
            ..Game::default()
        };
        game.computer_move();
        assert_eq!(game.turn, Player::White);
        assert_eq!(game.position, Position::initial());
        assert!(!game.dice.is_empty());
    }

    #[test]
    fn undo_is_committed_when_a_pass_and_play_turn_ends() {
        let mut game = Game {
            mode: Mode::PassAndPlay,
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
        game_action(&mut game, action_id("undo"));
        assert_eq!(game.turn, Player::Black);
        assert_eq!(game.message, "No move to undo.");
    }

    #[test]
    fn opening_cannot_double_and_match_length_restarts_everything() {
        let mut game = Game::default();
        game.offer_double();
        assert_eq!(game.phase, Phase::Playing);
        assert_eq!(game.message, "The opening roll must decide who starts.");

        game.position.points[23] = 0;
        game.cube = 8;
        game.cube_owner = Some(Player::White);
        game.opening = false;
        game.crawford_used = true;
        game.crawford_active = true;
        game.history.push(game.snapshot());
        game_action(&mut game, action_id("match"));
        assert_eq!(game.match_to, 7);
        assert_eq!(game.position, Position::initial());
        assert_eq!((game.cube, game.cube_owner), (1, None));
        assert!(!game.crawford_used && !game.crawford_active && game.history.is_empty());

        game.match_to = 7;
        game_action(&mut game, action_id("match"));
        assert_eq!(game.match_to, 1);
        assert!(!game.crawford_active);
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
        assert!(board_pixels(&game).contains(&32));
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

    #[test]
    fn deterministic_dice_has_equal_ordered_pair_counts() {
        let mut counts = [[0usize; 6]; 6];
        let mut game = Game::default();
        for _ in 0..3600 {
            let (first, second) = game.take_roll();
            counts[usize::from(first - 1)][usize::from(second - 1)] += 1;
        }
        assert!(counts.into_iter().flatten().all(|count| count == 100));
    }

    #[test]
    fn consumer_board_fits_clara_bw_without_diagnostics() {
        let mut game = Game::default();
        game.roll();
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("point-23")).is_some());
        assert!(layout.rect_of_action(action_id("computer")).is_none());
        let diagnostics = test_screen(&game).diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn board_uses_an_app_owned_picture_and_point_controls() {
        let mut game = Game::default();
        game.roll();
        let layout = test_screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::Picture(BOARD_PICTURE))));
        assert!(layout.rect_of_action(action_id("point-23")).is_some());
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
        assert_eq!((point_x(5), point_x(6)), (373, 518));
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
    fn picture_draws_board_checkers_dice_cube_and_legal_markers() {
        let mut game = Game::default();
        game.roll();
        let pixels = board_pixels(&game);
        assert_eq!(
            pixels[320 * usize::try_from(BOARD_WIDTH).expect("width") + 480],
            148
        );
        assert!(pixels.contains(&38));
        assert!(pixels.contains(&244));
        assert!(pixels.contains(&40));
        assert!(pixels.contains(&248));
    }
}
