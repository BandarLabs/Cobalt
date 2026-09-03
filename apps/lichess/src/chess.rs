//! Chess rules and position reconstruction.
//!
//! Lichess remains authoritative. This module is used to reject impossible
//! taps before a request is sent and to render only positions reconstructed
//! from server-acknowledged UCI moves.

use shakmaty::{
    fen::Fen,
    san::{San, SanPlus},
    uci::UciMove,
    CastlingMode, Chess, Color as ChessColor, EnPassantMode, Position,
};

pub const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Replayed {
    pub fen: String,
    pub last_san: Option<String>,
    pub check: bool,
}

pub fn normalize_initial(initial: &str) -> Option<String> {
    if initial.is_empty() || initial == "startpos" {
        Some(START.to_owned())
    } else {
        position(initial)
            .map(|position| Fen::from_position(position, EnPassantMode::Legal).to_string())
    }
}

pub fn legal(fen: &str, uci: &str) -> bool {
    move_for(fen, uci).is_some()
}

pub fn play(fen: &str, uci: &str) -> Option<(String, String)> {
    let position = position(fen)?;
    let movement = UciMove::from_ascii(uci.as_bytes())
        .ok()?
        .to_move(&position)
        .ok()?;
    let san = San::from_move(&position, &movement).to_string();
    let next = position.play(&movement).ok()?;
    Some((
        Fen::from_position(next, EnPassantMode::Legal).to_string(),
        san,
    ))
}

pub fn replay(initial: &str, moves: &[String]) -> Option<Replayed> {
    let mut fen = normalize_initial(initial)?;
    let mut last_san = None;
    for movement in moves {
        let (next, san) = play(&fen, movement)?;
        fen = next;
        last_san = Some(san);
    }
    Some(Replayed {
        check: position(&fen)?.is_check(),
        fen,
        last_san,
    })
}

pub fn side_to_move(fen: &str) -> Option<char> {
    match fen.split_whitespace().nth(1)? {
        "w" => Some('w'),
        "b" => Some('b'),
        _ => None,
    }
}

pub fn piece_at(fen: &str, square: &str) -> Option<char> {
    let bytes = square.as_bytes();
    if bytes.len() != 2 || !matches!(bytes[0], b'a'..=b'h') || !matches!(bytes[1], b'1'..=b'8') {
        return None;
    }
    let wanted_file = usize::from(bytes[0] - b'a');
    let wanted_rank = usize::from(b'8' - bytes[1]);
    for (rank_index, rank) in fen.split_whitespace().next()?.split('/').enumerate() {
        if rank_index != wanted_rank {
            continue;
        }
        let mut file = 0_usize;
        for character in rank.chars() {
            if let Some(empty) = character.to_digit(10) {
                file = file.saturating_add(usize::try_from(empty).ok()?);
            } else {
                if file == wanted_file {
                    return Some(character);
                }
                file = file.saturating_add(1);
            }
        }
        return None;
    }
    None
}

pub fn piece_belongs_to(fen: &str, square: &str, color: char) -> bool {
    piece_at(fen, square).is_some_and(|piece| match color {
        'w' => piece.is_ascii_uppercase(),
        'b' => piece.is_ascii_lowercase(),
        _ => false,
    })
}

pub fn promotion_choices(fen: &str, from: &str, to: &str) -> Vec<char> {
    let promotes = matches!(
        (piece_at(fen, from), to.as_bytes().get(1)),
        (Some('P'), Some(b'8')) | (Some('p'), Some(b'1'))
    );
    if !promotes {
        return Vec::new();
    }
    ['q', 'r', 'b', 'n']
        .into_iter()
        .filter(|promotion| legal(fen, &format!("{from}{to}{promotion}")))
        .collect()
}

pub fn tiny_move(fen: &str) -> Option<String> {
    const MAX_DEPTH: u8 = 4;
    const MAX_NODES: u32 = 5_000;
    let position = position(fen)?;
    let moves = ordered_moves(&position);
    let mut chosen = moves.first().map(|(uci, _)| uci.clone())?;
    for depth in 1..=MAX_DEPTH {
        let mut nodes = 0;
        let mut best: Option<(i32, String)> = None;
        let mut complete = true;
        for (uci, movement) in &moves {
            let next = position.clone().play(movement).ok()?;
            let Some(score) = tiny_search(
                &next,
                depth.saturating_sub(1),
                -100_000,
                100_000,
                &mut nodes,
                MAX_NODES,
            )
            .map(|score| -score) else {
                complete = false;
                break;
            };
            if best.as_ref().is_none_or(|(best_score, best_uci)| {
                score > *best_score || score == *best_score && uci < best_uci
            }) {
                best = Some((score, uci.clone()));
            }
        }
        if !complete {
            break;
        }
        if let Some((_, movement)) = best {
            chosen = movement;
        }
    }
    Some(chosen)
}

pub fn terminal(fen: &str) -> Option<(&'static str, Option<char>)> {
    let position = position(fen)?;
    if position.is_checkmate() {
        let winner = match side_to_move(fen)? {
            'w' => 'b',
            'b' => 'w',
            _ => return None,
        };
        Some(("mate", Some(winner)))
    } else if position.is_stalemate() || position.is_insufficient_material() {
        Some(("stalemate", None))
    } else {
        None
    }
}

pub fn has_mating_material(fen: &str, color: char) -> bool {
    let color = match color {
        'w' => ChessColor::White,
        'b' => ChessColor::Black,
        _ => return false,
    };
    position(fen).is_some_and(|position| !position.has_insufficient_material(color))
}

/// Replays normal PGN movetext through the move that creates a puzzle position.
pub fn puzzle_position(pgn: &str, initial_ply: usize) -> Option<String> {
    let puzzle_ply = initial_ply.checked_add(1)?;
    let mut position = Chess::default();
    let mut played = 0;
    for token in pgn.split_whitespace() {
        if token.starts_with('[')
            || token.ends_with(']')
            || token.contains('"')
            || token.ends_with('.')
            || token
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
            || matches!(token, "1-0" | "0-1" | "1/2-1/2" | "*")
        {
            continue;
        }
        if played == puzzle_ply {
            break;
        }
        let san = SanPlus::from_ascii(
            token
                .trim_matches(|character: char| character == '!' || character == '?')
                .as_bytes(),
        )
        .ok()?;
        let movement = san.san.to_move(&position).ok()?;
        position = position.play(&movement).ok()?;
        played += 1;
    }
    (played == puzzle_ply).then(|| Fen::from_position(position, EnPassantMode::Legal).to_string())
}

fn position(fen: &str) -> Option<Chess> {
    fen.parse::<Fen>()
        .ok()?
        .into_position(CastlingMode::Standard)
        .ok()
}

fn move_for(fen: &str, uci: &str) -> Option<shakmaty::Move> {
    let position = position(fen)?;
    UciMove::from_ascii(uci.as_bytes())
        .ok()?
        .to_move(&position)
        .ok()
}

fn tiny_search(
    position: &Chess,
    depth: u8,
    mut alpha: i32,
    beta: i32,
    nodes: &mut u32,
    max_nodes: u32,
) -> Option<i32> {
    if *nodes >= max_nodes {
        return None;
    }
    *nodes = nodes.saturating_add(1);
    if depth == 0 {
        return Some(evaluate(position));
    }
    let moves = position.legal_moves();
    if moves.is_empty() {
        return Some(if position.is_check() {
            -100_000 - i32::from(depth)
        } else {
            0
        });
    }
    let mut best = i32::MIN + 1;
    for (_, movement) in ordered_moves(position) {
        let Ok(next) = position.clone().play(&movement) else {
            continue;
        };
        let score = -tiny_search(&next, depth - 1, -beta, -alpha, nodes, max_nodes)?;
        best = best.max(score);
        alpha = alpha.max(score);
        if alpha >= beta {
            break;
        }
    }
    Some(best)
}

fn ordered_moves(position: &Chess) -> Vec<(String, shakmaty::Move)> {
    let mut moves = position
        .legal_moves()
        .into_iter()
        .filter_map(|movement| {
            let uci = movement.to_uci(CastlingMode::Standard).to_string();
            let next = position.clone().play(&movement).ok()?;
            Some((uci, movement, -evaluate(&next)))
        })
        .collect::<Vec<_>>();
    moves.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
    moves
        .into_iter()
        .map(|(uci, movement, _)| (uci, movement))
        .collect()
}

fn evaluate(position: &Chess) -> i32 {
    let fen = Fen::from_position(position.clone(), EnPassantMode::Legal).to_string();
    let mut score = 0;
    let mut rank = 0_i32;
    let mut file = 0_i32;
    for character in fen.split_whitespace().next().unwrap_or_default().chars() {
        if character == '/' {
            rank += 1;
            file = 0;
            continue;
        }
        if let Some(empty) = character.to_digit(10) {
            file += i32::try_from(empty).unwrap_or_default();
            continue;
        }
        let center = 14 - ((file * 2 - 7).abs() + (rank * 2 - 7).abs());
        let white = character.is_ascii_uppercase();
        let advancement = if white { 6 - rank } else { rank - 1 }.max(0);
        let value = match character.to_ascii_lowercase() {
            'p' => 100 + advancement * 8 + center * 2,
            'n' => 320 + center * 8,
            'b' => 330 + center * 4,
            'r' => 500 + center,
            'q' => 900 + center,
            'k' => {
                let castled = (white && rank == 7 || !white && rank == 0) && matches!(file, 2 | 6);
                i32::from(castled) * 35
            }
            _ => 0,
        };
        score += if white { value } else { -value };
        file += 1;
    }
    let mobility = i32::try_from(position.legal_moves().len()).unwrap_or_default() * 2;
    match side_to_move(&fen) {
        Some('w') => score + mobility,
        Some('b') => -score + mobility,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        legal, piece_at, piece_belongs_to, play, promotion_choices, puzzle_position, replay,
        terminal, tiny_move, START,
    };
    use shakmaty::{fen::Fen, perft, CastlingMode, Chess};

    #[test]
    fn standard_opening_moves_are_legal_and_replay_only_acknowledged_moves() {
        let (after, san) = play(START, "e2e4").expect("legal opening move");
        assert_eq!(san, "e4");
        assert!(legal(&after, "c7c5"));
        assert!(!legal(&after, "e2e5"));
        let replayed =
            replay("startpos", &["e2e4".to_owned(), "c7c5".to_owned()]).expect("server moves");
        assert_eq!(replayed.last_san.as_deref(), Some("c5"));
        assert_eq!(piece_at(&replayed.fen, "c5"), Some('p'));
    }

    #[test]
    fn special_moves_are_legal_only_when_the_server_position_allows_them() {
        let castle = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        assert!(legal(castle, "e1g1"));
        let en_passant = "8/8/8/3pP3/8/8/8/4K2k w - d6 0 1";
        assert!(legal(en_passant, "e5d6"));
        let promotion = "8/P7/8/8/8/8/8/4K2k w - - 0 1";
        assert_eq!(
            promotion_choices(promotion, "a7", "a8"),
            ['q', 'r', 'b', 'n']
        );
        let ordinary_capture = "4k3/8/5n2/4P3/8/8/8/4K3 w - - 0 1";
        assert!(promotion_choices(ordinary_capture, "e5", "f6").is_empty());
        let pinned = "4r2k/8/8/8/8/8/4N3/4K3 w - - 0 1";
        assert!(!legal(pinned, "e2c1"));
    }

    #[test]
    fn ownership_is_read_from_the_reconstructed_position() {
        assert!(piece_belongs_to(START, "e2", 'w'));
        assert!(!piece_belongs_to(START, "e7", 'w'));
    }

    #[test]
    fn perft_start_position_remains_the_rules_oracle() {
        let position: Chess = START
            .parse::<Fen>()
            .expect("fen")
            .into_position(CastlingMode::Standard)
            .expect("position");
        assert_eq!(perft(&position, 2), 400);
    }

    #[test]
    fn pgn_replay_includes_the_move_that_creates_the_puzzle() {
        let fen = puzzle_position("1. e4 e5 2. Nf3 Nc6", 2).expect("position");
        assert!(legal(&fen, "b8c6"));
    }

    #[test]
    fn tiny_engine_is_deterministic_and_terminal_positions_are_recognized() {
        let (after_e4, _) = play(START, "e2e4").expect("opening");
        let first = tiny_move(&after_e4).expect("move");
        assert_eq!(first, "g8f6");
        assert_eq!(tiny_move(&after_e4).as_deref(), Some(first.as_str()));
        assert_eq!(
            terminal("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1"),
            Some(("mate", Some('w')))
        );
    }
}
