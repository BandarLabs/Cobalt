//! Chess rules and position reconstruction.
//!
//! Lichess remains authoritative. This module is used to reject impossible
//! taps before a request is sent and to render only positions reconstructed
//! from server-acknowledged UCI moves.

use shakmaty::{
    fen::Fen,
    san::{San, SanPlus},
    uci::UciMove,
    CastlingMode, Chess, EnPassantMode, Position,
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
    ['q', 'r', 'b', 'n']
        .into_iter()
        .filter(|promotion| legal(fen, &format!("{from}{to}{promotion}")))
        .collect()
}

pub fn checked_king(fen: &str) -> Option<String> {
    if !position(fen)?.is_check() {
        return None;
    }
    let king = if side_to_move(fen)? == 'w' { 'K' } else { 'k' };
    square_of(fen, king)
}

/// Replays normal PGN movetext through the ply immediately before a puzzle.
pub fn puzzle_position(pgn: &str, initial_ply: usize) -> Option<String> {
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
        if played == initial_ply {
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
    (played == initial_ply).then(|| Fen::from_position(position, EnPassantMode::Legal).to_string())
}

fn square_of(fen: &str, wanted: char) -> Option<String> {
    for (rank_index, rank) in fen.split_whitespace().next()?.split('/').enumerate() {
        let mut file = 0_u8;
        for character in rank.chars() {
            if let Some(empty) = character.to_digit(10) {
                file = file.saturating_add(u8::try_from(empty).ok()?);
            } else {
                if character == wanted {
                    return Some(format!(
                        "{}{}",
                        char::from(b'a'.saturating_add(file)),
                        8_usize.saturating_sub(rank_index)
                    ));
                }
                file = file.saturating_add(1);
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::{
        checked_king, legal, piece_at, piece_belongs_to, play, promotion_choices, puzzle_position,
        replay, START,
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
        let pinned = "4r2k/8/8/8/8/8/4N3/4K3 w - - 0 1";
        assert!(!legal(pinned, "e2c1"));
    }

    #[test]
    fn ownership_and_check_are_read_from_the_reconstructed_position() {
        assert!(piece_belongs_to(START, "e2", 'w'));
        assert!(!piece_belongs_to(START, "e7", 'w'));
        let checked = "4k3/8/8/8/8/8/4r3/4K3 w - - 0 1";
        assert_eq!(checked_king(checked).as_deref(), Some("e1"));
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
