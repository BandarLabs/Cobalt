use std::fmt::Debug;

pub trait Game: Clone {
    type Move: Clone + Debug + Eq;

    fn side_to_move(&self) -> i8;
    fn legal_moves(&self) -> Vec<Self::Move>;
    fn apply(&self, mv: &Self::Move) -> Self;
    fn evaluate(&self, side: i8) -> i32;
    fn terminal_score(&self, side: i8) -> Option<i32>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Strength {
    Casual,
    Club,
    Strong,
}

impl Strength {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Casual => "Casual",
            Self::Club => "Club",
            Self::Strong => "Strong",
        }
    }

    const fn limits(self) -> (u8, usize) {
        match self {
            Self::Casual => (1, 500),
            Self::Club => (3, 5_000),
            Self::Strong => (5, 30_000),
        }
    }
}

pub fn choose_move<G: Game>(game: &G, strength: Strength) -> Option<G::Move> {
    let moves = game.legal_moves();
    let side = game.side_to_move();
    let (depth, limit) = strength.limits();
    let mut nodes = 0;
    let mut best = None;
    let mut best_score = i32::MIN;
    for mv in moves {
        let next = game.apply(&mv);
        let score = search(
            &next,
            depth.saturating_sub(1),
            side,
            i32::MIN + 1,
            i32::MAX,
            &mut nodes,
            limit,
        );
        if score > best_score {
            best_score = score;
            best = Some(mv);
        }
        if nodes >= limit {
            break;
        }
    }
    best
}

fn search<G: Game>(
    game: &G,
    depth: u8,
    root: i8,
    mut alpha: i32,
    mut beta: i32,
    nodes: &mut usize,
    limit: usize,
) -> i32 {
    *nodes += 1;
    if let Some(score) = game.terminal_score(root) {
        return score;
    }
    if depth == 0 || *nodes >= limit {
        return game.evaluate(root);
    }
    let maximizing = game.side_to_move() == root;
    if maximizing {
        let mut value = i32::MIN;
        for mv in game.legal_moves() {
            value = value.max(search(
                &game.apply(&mv),
                depth - 1,
                root,
                alpha,
                beta,
                nodes,
                limit,
            ));
            alpha = alpha.max(value);
            if alpha >= beta || *nodes >= limit {
                break;
            }
        }
        value
    } else {
        let mut value = i32::MAX;
        for mv in game.legal_moves() {
            value = value.min(search(
                &game.apply(&mv),
                depth - 1,
                root,
                alpha,
                beta,
                nodes,
                limit,
            ));
            beta = beta.min(value);
            if alpha >= beta || *nodes >= limit {
                break;
            }
        }
        value
    }
}

#[cfg(test)]
pub fn perft<G: Game>(game: &G, depth: u8) -> u64 {
    if depth == 0 {
        return 1;
    }
    game.legal_moves()
        .iter()
        .map(|mv| perft(&game.apply(mv), depth - 1))
        .sum()
}
