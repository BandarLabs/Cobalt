//! Complete, touch-first backgammon rules for a portrait Kobo panel.
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::cmp::Ordering;
use std::process::ExitCode;

const POINTS: usize = 24;
const CHECKERS: u8 = 15;
const SAVE: &str = "backgammon-autosave-v2";

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
            if self.all_home(player)
                && (target < 0 || target >= i16::try_from(POINTS).expect("point count fits i16"))
                && !self.has_checker_beyond(player, from)
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

struct Game {
    position: Position,
    turn: Player,
    dice: Vec<u8>,
    selected: Option<usize>,
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
}

impl Default for Game {
    fn default() -> Self {
        Self {
            position: Position::initial(),
            turn: Player::White,
            dice: Vec::new(),
            selected: None,
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
            self.dice.clear();
            self.turn = self.turn.other();
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

    fn select(&mut self, from: Option<usize>) {
        if self.dice.is_empty() {
            self.message = "Tap Roll first.".into();
            return;
        }
        if self.next_moves().iter().any(|play| play.from == from) {
            self.selected = from;
            self.message = "Checker selected. Tap a legal destination.".into();
        } else {
            self.message = "That checker has no legal point for this roll.".into();
        }
    }

    fn move_to(&mut self, to: Option<usize>) {
        let Some(play) = self
            .next_moves()
            .into_iter()
            .find(|play| play.from == self.selected && play.to == to)
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
        if self.position.won(self.turn) {
            self.finish_game(self.turn);
        } else if self.dice.is_empty()
            || legal_turns(&self.position, self.turn, &self.dice).is_empty()
        {
            self.dice.clear();
            self.turn = self.turn.other();
            self.message = format!("Move recorded. {} to roll.", self.turn.name());
        } else {
            self.message = "Use the remaining die.".into();
        }
    }

    fn offer_double(&mut self) {
        if self.phase != Phase::Playing || !self.dice.is_empty() {
            self.message = "Finish the roll before offering the cube.".into();
        } else if self.crawford_active {
            self.message = "The Crawford game has no cube.".into();
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
        self.cube = self.cube.saturating_mul(2);
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
        self.after_score(offered, "drops the cube");
    }

    fn finish_game(&mut self, winner: Player) {
        let earned = self.cube.saturating_mul(self.position.points_worth(winner));
        self.score[winner.index()] = self.score[winner.index()].saturating_add(earned);
        self.after_score(winner, "wins the game");
    }

    fn after_score(&mut self, winner: Player, result: &str) {
        self.message = format!("{} {result}.", winner.name());
        if self.score[winner.index()] >= self.match_to {
            self.phase = Phase::MatchOver(winner);
        } else {
            self.phase = Phase::GameOver(winner);
        }
    }

    fn start_game(&mut self) {
        self.crawford_active = !self.crawford_used
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

    fn computer_move(&mut self) {
        if self.mode != Mode::Solo || self.turn != Player::Black || self.phase != Phase::Playing {
            return;
        }
        if self.dice.is_empty() {
            self.roll();
        }
        while !self.dice.is_empty() && self.phase == Phase::Playing {
            let mut choices = self.next_moves();
            choices.sort_by_key(|play| std::cmp::Reverse(self.score_after(*play)));
            if let Some(play) = choices.first().copied() {
                self.selected = play.from;
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
            "{points};{},{};{},{};{};{};{};{};{},{};{};{};{};{};{};{}",
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
            u8::from(self.opening)
        )
    }

    fn decode(text: &str) -> Option<Self> {
        let fields: Vec<_> = text.split(';').collect();
        if fields.len() != 14 {
            return None;
        }
        let point_values: Vec<_> = fields[0]
            .split(',')
            .map(str::parse::<i8>)
            .collect::<Result<_, _>>()
            .ok()?;
        if point_values.len() != POINTS {
            return None;
        }
        let pair = |field: &str| -> Option<[u8; 2]> {
            let values: Vec<_> = field
                .split(',')
                .map(str::parse::<u8>)
                .collect::<Result<_, _>>()
                .ok()?;
            (values.len() == 2).then_some([values[0], values[1]])
        };
        let mut points = [0; POINTS];
        points.copy_from_slice(&point_values);
        let turn = match fields[3] {
            "0" => Player::White,
            "1" => Player::Black,
            _ => return None,
        };
        let dice = if fields[4].is_empty() {
            Vec::new()
        } else {
            fields[4]
                .split(',')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?
        };
        if dice.iter().any(|die: &u8| !(1..=6).contains(die)) {
            return None;
        }
        let owner = match fields[6] {
            "0" => Some(Player::White),
            "1" => Some(Player::Black),
            "2" => None,
            _ => return None,
        };
        let mode = match fields[11] {
            "0" => Mode::Solo,
            "1" => Mode::PassAndPlay,
            _ => return None,
        };
        let position = Position {
            points,
            bar: pair(fields[1])?,
            off: pair(fields[2])?,
        };
        (position
            .bar
            .iter()
            .chain(position.off.iter())
            .all(|count| *count <= CHECKERS))
        .then_some(Self {
            position,
            turn,
            dice,
            selected: None,
            cube: fields[5].parse().ok()?,
            cube_owner: owner,
            score: pair(fields[7])?,
            match_to: fields[8].parse().ok()?,
            crawford_used: fields[9] == "1",
            crawford_active: fields[10] == "1",
            mode,
            rolls: fields[12].parse().ok()?,
            opening: fields[13] == "1",
            phase: Phase::Playing,
            history: Vec::new(),
            message: "Resumed saved game.".into(),
        })
    }
}

fn point_name(point: usize) -> String {
    format!("point-{point}")
}

fn screen(game: &Game) -> Screen {
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
        Phase::GameOver(winner) => ScreenBuilder::new("backgammon-game-over")
            .top_bar("Backgammon")
            .secondary(format!("{} {}.", winner.name(), game.message))
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
        Phase::Playing => playing_screen(game),
    }
}

fn playing_screen(game: &Game) -> Screen {
    let order: Vec<_> = if game.mode == Mode::PassAndPlay && game.turn == Player::Black {
        (0..POINTS).rev().collect()
    } else {
        (0..POINTS).collect()
    };
    let points = order.into_iter().map(|point| {
        let checkers = game.position.points[point];
        let count = checkers.unsigned_abs();
        let (colour, glyph) = match checkers.cmp(&0) {
            Ordering::Greater => ("white", Some(Glyph::WhiteDisc)),
            Ordering::Less => ("black", Some(Glyph::BlackDisc)),
            Ordering::Equal => ("empty", None),
        };
        (
            point_name(point),
            format!(
                "Point {} {colour} {count} checker{}",
                point + 1,
                if count == 1 { "" } else { "s" }
            ),
            glyph,
        )
    });
    let dice = if game.dice.is_empty() {
        "—".into()
    } else {
        game.dice
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let bar_active = game.next_moves().iter().any(|play| play.from.is_none());
    let bar = checker_counter("Bar", game.position.bar);
    let bar = if bar_active {
        format!("○ {bar}")
    } else {
        bar
    };
    ScreenBuilder::new("backgammon")
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
        .secondary(&game.message)
        .grid(
            2,
            false,
            [
                ("bar", bar),
                ("off", checker_counter("Off", game.position.off)),
            ],
        )
        .board(12, points)
        .grid(
            3,
            false,
            [("roll", "Roll"), ("double", "Double"), ("undo", "Undo")],
        )
        .grid(
            3,
            false,
            [
                ("mode", game.mode.label().to_owned()),
                ("match", format!("Match: {}", game.match_to)),
                ("computer", "Computer move".to_owned()),
            ],
        )
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
        context.set_screen(screen(self));
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let mut changed = true;
        match game_action(self, action) {
            Some(()) => {}
            None => changed = false,
        }
        if changed {
            context.store().save(SAVE, self.encode());
            context.set_screen(screen(self));
        }
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded {
            key,
            value: Some(value),
        } = result
        {
            if key == SAVE {
                if let Ok(text) = String::from_utf8(value) {
                    if let Some(saved) = Self::decode(&text) {
                        *self = saved;
                    }
                }
                context.set_screen(screen(self));
            }
        }
    }
}

fn game_action(game: &mut Game, action: ActionId) -> Option<()> {
    match game.phase {
        Phase::ConfirmDouble if action == action_id("confirm-double") => game.confirm_double(),
        Phase::ConfirmDouble if action == action_id("cancel-double") => game.phase = Phase::Playing,
        Phase::Offered(player) if action == action_id("take") => game.take(player),
        Phase::Offered(player) if action == action_id("drop") => {
            game.phase = Phase::ConfirmDrop(player);
        }
        Phase::ConfirmDrop(player) if action == action_id("confirm-drop") => game.drop_cube(player),
        Phase::ConfirmDrop(_) if action == action_id("cancel-drop") => {
            game.phase = Phase::Offered(game.turn);
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
            game.match_to = match game.match_to {
                1 => 3,
                3 => 5,
                5 => 7,
                _ => 1,
            };
            game.message = format!("Match to {}.", game.match_to);
        }
        Phase::Playing if action == action_id("computer") => game.computer_move(),
        Phase::Playing if action == action_id("bar") => game.select(None),
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
    use kobo_ui::{render_with, tone, Chrome, LayoutKind, Surface, CLARA_BW_METRICS};

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
        let mut game = Game::default();
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
    fn autosave_round_trips_position_and_match_state() {
        let mut game = Game::default();
        game.position.bar = [1, 2];
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
        let game = Game::default();
        let layout = screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("point-23")).is_some());
        assert!(layout.rect_of_action(action_id("computer")).is_some());
        let diagnostics = screen(&game).diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn board_uses_triangular_points_and_checker_stacks() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::BackgammonBoard)));
        assert_eq!(
            layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::BackgammonStack(..)))
                .count(),
            8
        );
    }

    #[test]
    fn pass_and_play_reverses_the_point_order() {
        let game = Game {
            mode: Mode::PassAndPlay,
            turn: Player::Black,
            ..Game::default()
        };
        let layout = screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let first = layout
            .rect_of_action(action_id("point-23"))
            .expect("Black-nearest point is drawn");
        let last = layout
            .rect_of_action(action_id("point-0"))
            .expect("opposite point is drawn");
        assert!(first.y < last.y);
    }

    #[test]
    fn renderer_draws_the_central_bar_and_vector_checker_ink() {
        let game = Game::default();
        let screen = screen(&game);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let board = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::BackgammonBoard))
            .expect("backgammon board");
        let mut surface = Surface::new(
            usize::try_from(CLARA_BW_METRICS.width).expect("panel width"),
            usize::try_from(CLARA_BW_METRICS.height).expect("panel height"),
        );
        render_with(
            &screen,
            &CLARA_BW_METRICS,
            &Chrome::default(),
            &mut surface,
            None,
        );
        let middle = usize::try_from(board.rect.y + board.rect.height / 2).expect("bar row")
            * surface.width
            + usize::try_from(board.rect.x + board.rect.width / 2).expect("bar column");
        assert_eq!(surface.pixels[middle], tone::MUTED);
        assert!(surface.pixels.contains(&tone::INK));
    }
}
