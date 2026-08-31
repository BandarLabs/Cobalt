//! The only module that knows about `shakmaty`.
//!
//! Keeping rules here means network and UI code only exchange FEN and UCI.
//! This is also where Lichess' PGN is replayed to its `initialPly` position.

use shakmaty::{
    fen::Fen,
    san::{San, SanPlus},
    uci::UciMove,
    CastlingMode, Chess, EnPassantMode, Position,
};

#[cfg(test)]
pub const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

pub fn legal(fen: &str, uci: &str) -> bool {
    move_for(fen, uci).is_some()
}

pub fn play(fen: &str, uci: &str) -> Option<(String, String)> {
    let position = position(fen)?;
    let movement = UciMove::from_ascii(uci.as_bytes()).ok()?.to_move(&position).ok()?;
    let san = San::from_move(&position, &movement).to_string();
    let next = position.play(&movement).ok()?;
    Some((
        Fen::from_position(next, EnPassantMode::Legal).to_string(),
        san,
    ))
}

/// Replays a normal PGN movetext through the ply immediately before the puzzle.
pub fn puzzle_position(pgn: &str, initial_ply: usize) -> Option<String> {
    let mut position = Chess::default();
    let mut played = 0;
    for token in pgn.split_whitespace() {
        if token.starts_with('[')
            || token.ends_with(']')
            || token.contains('"')
            || token.ends_with('.')
            || token.chars().all(|character| character.is_ascii_digit() || character == '.')
            || matches!(token, "1-0" | "0-1" | "1/2-1/2" | "*")
        {
            continue;
        }
        if played == initial_ply {
            break;
        }
        let san = SanPlus::from_ascii(token.trim_matches(|c: char| c == '!' || c == '?').as_bytes())
            .ok()?;
        let movement = san.san.to_move(&position).ok()?;
        position = position.play(&movement).ok()?;
        played += 1;
    }
    (played == initial_ply).then(|| Fen::from_position(position, EnPassantMode::Legal).to_string())
}

fn position(fen: &str) -> Option<Chess> {
    fen.parse::<Fen>()
        .ok()?
        .into_position(CastlingMode::Standard)
        .ok()
}

fn move_for(fen: &str, uci: &str) -> Option<shakmaty::Move> {
    let position = position(fen)?;
    UciMove::from_ascii(uci.as_bytes()).ok()?.to_move(&position).ok()
}

#[cfg(test)]
mod tests {
    use super::{legal, play, puzzle_position, START};
    use shakmaty::{fen::Fen, perft, CastlingMode, Chess};

    #[test]
    fn standard_opening_moves_are_legal_and_render_as_san() {
        let (after, san) = play(START, "e2e4").expect("legal opening move");
        assert_eq!(san, "e4");
        assert!(legal(&after, "c7c5"));
        assert!(!legal(&after, "e2e5"));
    }

    #[test]
    fn special_moves_are_legal_only_when_the_position_allows_them() {
        let castle = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        assert!(legal(castle, "e1g1"));
        let en_passant = "8/8/8/3pP3/8/8/8/4K2k w - d6 0 1";
        assert!(legal(en_passant, "e5d6"));
        let promotion = "8/P7/8/8/8/8/8/4K2k w - - 0 1";
        assert!(legal(promotion, "a7a8n"));
        let pinned = "4r2k/8/8/8/8/8/4N3/4K3 w - - 0 1";
        assert!(!legal(pinned, "e2c1"));
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
    fn pgn_replay_stops_at_the_puzzle_ply() {
        let fen = puzzle_position("1. e4 e5 2. Nf3 Nc6", 3).expect("position");
        assert!(legal(&fen, "b8c6"));
    }
}
