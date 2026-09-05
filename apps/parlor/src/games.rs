#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use crate::engine::Game;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rules {
    AngloAmerican,
    International,
}

impl Rules {
    pub const fn name(self) -> &'static str {
        match self {
            Self::AngloAmerican => "Anglo-American (8×8)",
            Self::International => "International (10×10)",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reversi {
    pub board: [i8; 64],
    pub turn: i8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReversiMove {
    Place(usize),
    Pass,
}

impl Default for Reversi {
    fn default() -> Self {
        let mut board = [0; 64];
        board[27] = -1;
        board[28] = 1;
        board[35] = 1;
        board[36] = -1;
        Self { board, turn: 1 }
    }
}

impl Reversi {
    pub fn flips(&self, at: usize) -> Vec<usize> {
        if at >= 64 || self.board[at] != 0 {
            return Vec::new();
        }
        let (row, col) = (at / 8, at % 8);
        let mut all = Vec::new();
        for (dr, dc) in [
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ] {
            let (mut r, mut c) = (row as isize + dr, col as isize + dc);
            let mut run = Vec::new();
            while (0..8).contains(&r) && (0..8).contains(&c) {
                let square = r as usize * 8 + c as usize;
                match self.board[square] {
                    x if x == -self.turn => run.push(square),
                    x if x == self.turn => {
                        all.extend(run);
                        break;
                    }
                    _ => break,
                }
                r += dr;
                c += dc;
            }
        }
        all
    }

    pub fn placements(&self) -> Vec<usize> {
        (0..64).filter(|&at| !self.flips(at).is_empty()).collect()
    }
}

impl Game for Reversi {
    type Move = ReversiMove;
    fn side_to_move(&self) -> i8 {
        self.turn
    }
    fn legal_moves(&self) -> Vec<Self::Move> {
        let places = self.placements();
        if places.is_empty() {
            vec![ReversiMove::Pass]
        } else {
            places.into_iter().map(ReversiMove::Place).collect()
        }
    }
    fn apply(&self, mv: &Self::Move) -> Self {
        let mut next = self.clone();
        if let ReversiMove::Place(at) = *mv {
            next.board[at] = self.turn;
            for square in self.flips(at) {
                next.board[square] = self.turn;
            }
        }
        next.turn = -self.turn;
        next
    }
    fn evaluate(&self, side: i8) -> i32 {
        let discs = self
            .board
            .iter()
            .map(|&x| i32::from(x) * i32::from(side))
            .sum::<i32>();
        let corners = [0, 7, 56, 63]
            .iter()
            .map(|&i| i32::from(self.board[i]) * i32::from(side))
            .sum::<i32>();
        let mobility = self.placements().len() as i32;
        discs
            + corners * 18
            + if self.turn == side {
                mobility
            } else {
                -mobility
            }
    }
    fn terminal_score(&self, side: i8) -> Option<i32> {
        if !self.placements().is_empty() {
            return None;
        }
        let mut other = self.clone();
        other.turn = -other.turn;
        if !other.placements().is_empty() {
            return None;
        }
        let difference = self
            .board
            .iter()
            .map(|&piece| i32::from(piece) * i32::from(side))
            .sum::<i32>();
        Some(difference.signum() * 100_000)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Draughts {
    pub rules: Rules,
    pub board: Vec<i8>,
    pub turn: i8,
    pub forced: Option<usize>,
    pub captured: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraughtsMove {
    pub from: usize,
    pub to: usize,
    pub captured: Option<usize>,
}

impl Draughts {
    pub fn new(rules: Rules) -> Self {
        let side = if rules == Rules::International { 10 } else { 8 };
        let rows = if side == 10 { 4 } else { 3 };
        let mut board = vec![0; side * side];
        for row in 0..rows {
            for col in 0..side {
                if (row + col) % 2 == 1 {
                    board[row * side + col] = -1;
                }
            }
        }
        for row in side - rows..side {
            for col in 0..side {
                if (row + col) % 2 == 1 {
                    board[row * side + col] = 1;
                }
            }
        }
        Self {
            rules,
            board,
            turn: 1,
            forced: None,
            captured: Vec::new(),
        }
    }

    pub fn side(&self) -> usize {
        if self.rules == Rules::International {
            10
        } else {
            8
        }
    }

    fn jumps_from(&self, from: usize) -> Vec<DraughtsMove> {
        let side = self.side() as isize;
        let piece = self.board[from];
        if piece.signum() != self.turn {
            return Vec::new();
        }
        let (row, col) = ((from / self.side()) as isize, (from % self.side()) as isize);
        let mut result = Vec::new();
        let dirs = [(-1, -1), (-1, 1), (1, -1), (1, 1)];
        if piece.abs() == 2 && self.rules == Rules::International {
            for (dr, dc) in dirs {
                let (mut r, mut c) = (row + dr, col + dc);
                let mut victim = None;
                while (0..side).contains(&r) && (0..side).contains(&c) {
                    let at = r as usize * self.side() + c as usize;
                    if self.captured.contains(&at) {
                        break;
                    }
                    if self.board[at] == 0 {
                        if let Some(captured) = victim {
                            result.push(DraughtsMove {
                                from,
                                to: at,
                                captured: Some(captured),
                            });
                        }
                    } else if self.board[at].signum() == self.turn || victim.is_some() {
                        break;
                    } else {
                        victim = Some(at);
                    }
                    r += dr;
                    c += dc;
                }
            }
        } else {
            let short_dirs: &[(isize, isize)] =
                if piece.abs() == 1 && self.rules == Rules::AngloAmerican {
                    if self.turn == 1 {
                        &[(-1, -1), (-1, 1)]
                    } else {
                        &[(1, -1), (1, 1)]
                    }
                } else {
                    &dirs
                };
            for &(dr, dc) in short_dirs {
                let (mr, mc, tr, tc) = (row + dr, col + dc, row + 2 * dr, col + 2 * dc);
                if (0..side).contains(&tr) && (0..side).contains(&tc) {
                    let mid = mr as usize * self.side() + mc as usize;
                    let to = tr as usize * self.side() + tc as usize;
                    if !self.captured.contains(&mid)
                        && self.board[mid].signum() == -self.turn
                        && self.board[to] == 0
                    {
                        result.push(DraughtsMove {
                            from,
                            to,
                            captured: Some(mid),
                        });
                    }
                }
            }
        }
        result
    }

    pub(crate) fn capture_length(&self, mv: &DraughtsMove) -> usize {
        let mut next = self.clone();
        let piece = next.board[mv.from];
        next.board[mv.from] = 0;
        next.board[mv.to] = piece;
        if let Some(captured) = mv.captured {
            if self.rules == Rules::International {
                next.captured.push(captured);
            } else {
                next.board[captured] = 0;
            }
        }
        if self.rules == Rules::International {
            next.crown_at(mv.to);
        }
        let continuations = next.jumps_from(mv.to);
        1 + continuations
            .iter()
            .map(|next_move| next.capture_length(next_move))
            .max()
            .unwrap_or(0)
    }

    fn required_captures(&self, captures: Vec<DraughtsMove>) -> Vec<DraughtsMove> {
        if self.rules != Rules::International || captures.is_empty() {
            return captures;
        }
        let longest = captures
            .iter()
            .map(|mv| self.capture_length(mv))
            .max()
            .unwrap_or(0);
        captures
            .into_iter()
            .filter(|mv| self.capture_length(mv) == longest)
            .collect()
    }

    fn steps_from(&self, from: usize) -> Vec<DraughtsMove> {
        let side = self.side() as isize;
        let piece = self.board[from];
        if piece.signum() != self.turn {
            return Vec::new();
        }
        let (row, col) = ((from / self.side()) as isize, (from % self.side()) as isize);
        let dirs = if piece.abs() == 2 {
            vec![(-1, -1), (-1, 1), (1, -1), (1, 1)]
        } else if self.turn == 1 {
            vec![(-1, -1), (-1, 1)]
        } else {
            vec![(1, -1), (1, 1)]
        };
        let mut result = Vec::new();
        for (dr, dc) in dirs {
            let (mut r, mut c) = (row + dr, col + dc);
            while (0..side).contains(&r) && (0..side).contains(&c) {
                let to = r as usize * self.side() + c as usize;
                if self.board[to] != 0 {
                    break;
                }
                result.push(DraughtsMove {
                    from,
                    to,
                    captured: None,
                });
                if piece.abs() != 2 || self.rules == Rules::AngloAmerican {
                    break;
                }
                r += dr;
                c += dc;
            }
        }
        result
    }

    pub fn moves(&self) -> Vec<DraughtsMove> {
        if let Some(from) = self.forced {
            let captures = self.jumps_from(from);
            return self.required_captures(captures);
        }
        let captures: Vec<_> = (0..self.board.len())
            .flat_map(|from| self.jumps_from(from))
            .collect();
        if captures.is_empty() {
            (0..self.board.len())
                .flat_map(|from| self.steps_from(from))
                .collect()
        } else {
            self.required_captures(captures)
        }
    }

    pub fn apply_move(&self, mv: &DraughtsMove) -> Self {
        let mut next = self.clone();
        let piece = next.board[mv.from];
        next.board[mv.from] = 0;
        next.board[mv.to] = piece;
        if let Some(captured) = mv.captured {
            if self.rules == Rules::International {
                next.captured.push(captured);
            } else {
                next.board[captured] = 0;
            }
            if next.rules == Rules::International {
                next.crown_at(mv.to);
            }
            next.forced = Some(mv.to);
            if next.jumps_from(mv.to).is_empty() {
                for captured in next.captured.drain(..) {
                    next.board[captured] = 0;
                }
                next.forced = None;
                next.crown_at(mv.to);
                next.turn = -next.turn;
            }
        } else {
            next.crown_at(mv.to);
            next.turn = -next.turn;
            next.forced = None;
            next.captured.clear();
        }
        next
    }

    fn crown_at(&mut self, at: usize) {
        let piece = self.board[at];
        let row = at / self.side();
        if (piece == 1 && row == 0) || (piece == -1 && row + 1 == self.side()) {
            self.board[at] = piece * 2;
        }
    }
}

impl Game for Draughts {
    type Move = DraughtsMove;
    fn side_to_move(&self) -> i8 {
        self.turn
    }
    fn legal_moves(&self) -> Vec<Self::Move> {
        self.moves()
    }
    fn apply(&self, mv: &Self::Move) -> Self {
        self.apply_move(mv)
    }
    fn evaluate(&self, side: i8) -> i32 {
        self.board
            .iter()
            .map(|&p| i32::from(p.signum() * side) * if p.abs() == 2 { 5 } else { 3 })
            .sum()
    }
    fn terminal_score(&self, side: i8) -> Option<i32> {
        self.moves()
            .is_empty()
            .then_some(if self.turn == side { -100_000 } else { 100_000 })
    }
}

const EDGES: &[(usize, usize)] = &[
    (0, 1),
    (1, 2),
    (2, 14),
    (14, 23),
    (23, 22),
    (22, 21),
    (21, 9),
    (9, 0),
    (3, 4),
    (4, 5),
    (5, 13),
    (13, 20),
    (20, 19),
    (19, 18),
    (18, 10),
    (10, 3),
    (6, 7),
    (7, 8),
    (8, 12),
    (12, 17),
    (17, 16),
    (16, 15),
    (15, 11),
    (11, 6),
    (1, 4),
    (4, 7),
    (14, 13),
    (13, 12),
    (22, 19),
    (19, 16),
    (9, 10),
    (10, 11),
];
const MILLS: [[usize; 3]; 16] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [9, 10, 11],
    [12, 13, 14],
    [15, 16, 17],
    [18, 19, 20],
    [21, 22, 23],
    [0, 9, 21],
    [3, 10, 18],
    [6, 11, 15],
    [1, 4, 7],
    [16, 19, 22],
    [8, 12, 17],
    [5, 13, 20],
    [2, 14, 23],
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Morris {
    pub board: [i8; 24],
    pub turn: i8,
    pub placed: [u8; 2],
    pub removing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MorrisMove {
    Place(usize),
    Slide(usize, usize),
    Remove(usize),
}

impl Default for Morris {
    fn default() -> Self {
        Self {
            board: [0; 24],
            turn: 1,
            placed: [0, 0],
            removing: false,
        }
    }
}

impl Morris {
    fn index(side: i8) -> usize {
        usize::from(side != 1)
    }
    pub fn in_mill(&self, at: usize, side: i8) -> bool {
        MILLS
            .iter()
            .any(|mill| mill.contains(&at) && mill.iter().all(|&p| self.board[p] == side))
    }
    pub fn count(&self, side: i8) -> usize {
        self.board.iter().filter(|&&p| p == side).count()
    }
    pub fn moves(&self) -> Vec<MorrisMove> {
        if self.removing {
            let mut pieces: Vec<_> = (0..24)
                .filter(|&p| self.board[p] == -self.turn && !self.in_mill(p, -self.turn))
                .collect();
            if pieces.is_empty() {
                pieces = (0..24).filter(|&p| self.board[p] == -self.turn).collect();
            }
            return pieces.into_iter().map(MorrisMove::Remove).collect();
        }
        if self.placed[Self::index(self.turn)] < 9 {
            return (0..24)
                .filter(|&p| self.board[p] == 0)
                .map(MorrisMove::Place)
                .collect();
        }
        let flying = self.count(self.turn) == 3;
        let mut result = Vec::new();
        for from in (0..24).filter(|&p| self.board[p] == self.turn) {
            for to in (0..24).filter(|&p| self.board[p] == 0) {
                if flying || EDGES.contains(&(from, to)) || EDGES.contains(&(to, from)) {
                    result.push(MorrisMove::Slide(from, to));
                }
            }
        }
        result
    }
    pub fn apply_move(&self, mv: &MorrisMove) -> Self {
        let mut next = self.clone();
        let destination = match *mv {
            MorrisMove::Place(to) => {
                next.board[to] = self.turn;
                next.placed[Self::index(self.turn)] += 1;
                Some(to)
            }
            MorrisMove::Slide(from, to) => {
                next.board[from] = 0;
                next.board[to] = self.turn;
                Some(to)
            }
            MorrisMove::Remove(at) => {
                next.board[at] = 0;
                next.removing = false;
                next.turn = -self.turn;
                None
            }
        };
        if let Some(to) = destination {
            if next.in_mill(to, self.turn) {
                next.removing = true;
            } else {
                next.turn = -self.turn;
            }
        }
        next
    }
}

impl Game for Morris {
    type Move = MorrisMove;
    fn side_to_move(&self) -> i8 {
        self.turn
    }
    fn legal_moves(&self) -> Vec<Self::Move> {
        self.moves()
    }
    fn apply(&self, mv: &Self::Move) -> Self {
        self.apply_move(mv)
    }
    fn evaluate(&self, side: i8) -> i32 {
        let pieces = (self.count(side) as i32 - self.count(-side) as i32) * 10;
        let mills = MILLS
            .iter()
            .map(|m| {
                let sum: i8 = m.iter().map(|&p| self.board[p]).sum();
                if sum == side * 2 {
                    3
                } else if sum == -side * 2 {
                    -3
                } else {
                    0
                }
            })
            .sum::<i32>();
        pieces + mills
    }
    fn terminal_score(&self, side: i8) -> Option<i32> {
        if self.placed != [9, 9] || self.removing {
            return None;
        }
        if self.count(self.turn) < 3 || self.moves().is_empty() {
            Some(if self.turn == side { -100_000 } else { 100_000 })
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kalah {
    pub pits: [u8; 14],
    pub turn: i8,
}

impl Default for Kalah {
    fn default() -> Self {
        let mut pits = [4; 14];
        pits[6] = 0;
        pits[13] = 0;
        Self { pits, turn: 1 }
    }
}

impl Kalah {
    pub fn own_pits(&self) -> std::ops::Range<usize> {
        if self.turn == 1 {
            0..6
        } else {
            7..13
        }
    }
    pub fn moves(&self) -> Vec<usize> {
        self.own_pits().filter(|&p| self.pits[p] > 0).collect()
    }
    pub fn apply_move(&self, pit: usize) -> Self {
        let mut next = self.clone();
        let mut seeds = next.pits[pit];
        next.pits[pit] = 0;
        let opponent_store = if self.turn == 1 { 13 } else { 6 };
        let own_store = if self.turn == 1 { 6 } else { 13 };
        let mut at = pit;
        while seeds > 0 {
            at = (at + 1) % 14;
            if at == opponent_store {
                continue;
            }
            next.pits[at] += 1;
            seeds -= 1;
        }
        let own_range = if self.turn == 1 { 0..6 } else { 7..13 };
        if own_range.contains(&at) && next.pits[at] == 1 {
            let opposite = 12 - at;
            if next.pits[opposite] > 0 {
                next.pits[own_store] += next.pits[opposite] + 1;
                next.pits[opposite] = 0;
                next.pits[at] = 0;
            }
        }
        if at != own_store {
            next.turn = -self.turn;
        }
        let south_empty = next.pits[0..6].iter().all(|&n| n == 0);
        let north_empty = next.pits[7..13].iter().all(|&n| n == 0);
        if south_empty || north_empty {
            let south: u8 = next.pits[0..6].iter().sum();
            let north: u8 = next.pits[7..13].iter().sum();
            next.pits[6] += south;
            next.pits[13] += north;
            next.pits[0..6].fill(0);
            next.pits[7..13].fill(0);
        }
        next
    }
}

impl Game for Kalah {
    type Move = usize;
    fn side_to_move(&self) -> i8 {
        self.turn
    }
    fn legal_moves(&self) -> Vec<Self::Move> {
        self.moves()
    }
    fn apply(&self, mv: &Self::Move) -> Self {
        self.apply_move(*mv)
    }
    fn evaluate(&self, side: i8) -> i32 {
        let score = i32::from(self.pits[6]) - i32::from(self.pits[13]);
        score * i32::from(side)
    }
    fn terminal_score(&self, side: i8) -> Option<i32> {
        if self.moves().is_empty() {
            let difference = i32::from(self.pits[6]) - i32::from(self.pits[13]);
            Some((difference * i32::from(side)).signum() * 100_000)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Draughts, DraughtsMove, Rules};

    #[test]
    fn international_man_crowns_during_a_capture_and_continues_as_a_king() {
        let mut game = Draughts::new(Rules::International);
        game.board.fill(0);
        game.board[21] = 1;
        game.board[12] = -1;
        game.board[25] = -1;
        let first = DraughtsMove {
            from: 21,
            to: 3,
            captured: Some(12),
        };
        assert!(game.moves().contains(&first));

        let next = game.apply_move(&first);
        assert_eq!(next.board[3], 2);
        assert_eq!(next.forced, Some(3));
        assert!(next.moves().iter().any(|mv| mv.captured == Some(25)));
    }
}
