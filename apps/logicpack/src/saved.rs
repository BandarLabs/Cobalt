//! Versioned progress and undo for stable original collection identities.
//!
//! Version 3 keeps a record only for puzzles somebody has played and names
//! each by its id. Version 2 carried a record for every puzzle in collection
//! order and refused a file whose count differed, which would have broken
//! every save the day the collection grew, and a record per puzzle for four
//! hundred puzzles would not fit the store's limit anyway.
use crate::{collection, Game, Kind};
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};
use std::collections::{BTreeMap, BTreeSet};
pub const LIMIT: usize = 128 * 1024;
pub const HISTORY: usize = 32;
/// How many puzzles existed when progress was last saved as version 2.
const VERSION_TWO_PUZZLES: usize = 20;
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct Run {
    pub position: Vec<u8>,
    pub undo: Vec<Vec<u8>>,
}
fn schema() -> Schema {
    Schema::new("logicpack.games", 3, LIMIT).expect("fixed schema")
}
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("ASCII snapshot")
}
fn record(id: &str, position: &[u8], undo: &[Vec<u8>]) -> Value {
    ObjectBuilder::new()
        .set("id", id)
        .set("position", text(position))
        .set(
            "undo",
            Value::Array(undo.iter().map(|p| Value::String(text(p).into())).collect()),
        )
        .build()
}
/// The undo steps a puzzle keeps on disk: all of them for the open puzzle,
/// the newest `keep` for any other.
fn kept(run: &Run, open: bool, keep: usize) -> &[Vec<u8>] {
    if open {
        &run.undo
    } else {
        &run.undo[run.undo.len().saturating_sub(keep)..]
    }
}
pub fn encode(games: &[Run], current: Option<usize>) -> Vec<u8> {
    let played = || {
        games
            .iter()
            .zip(collection::puzzles())
            .enumerate()
            .filter(|(_, (run, _))| !run.position.is_empty())
            .map(|(index, (run, p))| (Some(index) == current, run, p))
    };
    // Puzzles that are not open give up their oldest undo steps, then all of
    // them, only when somebody has so many puzzles under way that the record
    // would not fit otherwise: progress matters more than the undo trail of
    // a puzzle nobody is looking at.
    for keep in [HISTORY, 16, 8, 4, 2, 1, 0] {
        let estimate: usize = played()
            .map(|(open, run, p)| {
                let undo = kept(run, open, keep).iter().map(|u| u.len() + 3);
                48 + p.id.len() + run.position.len() + undo.sum::<usize>()
            })
            .sum();
        if keep > 0 && estimate + 256 > LIMIT {
            continue;
        }
        let payload = ObjectBuilder::new()
            .set(
                "current",
                current.map_or(Value::Null, |i| {
                    Value::String(collection::puzzles()[i].id.clone())
                }),
            )
            .set(
                "games",
                Value::Array(
                    played()
                        .map(|(open, run, p)| record(&p.id, &run.position, kept(run, open, keep)))
                        .collect(),
                ),
            )
            .build();
        match schema().encode(&payload) {
            Ok(bytes) => return bytes,
            Err(Error::TooLarge) if keep > 0 => {}
            Err(error) => panic!("bounded collection: {error}"),
        }
    }
    unreachable!("the last attempt returns or panics")
}
fn canonical(bytes: &[u8], index: usize) -> Option<Vec<u8>> {
    if bytes.len() > 192 {
        return None;
    }
    let mut game = Game::blank();
    (game.restore(bytes) && game.kind != Kind::Home && game.puzzle == index).then(|| game.encode())
}
fn migrate(version: u32, payload: &Value) -> Result<Value, Error> {
    match version {
        1 => first_collection(payload),
        2 => by_id(payload),
        _ => Err(Error::MigrationUnavailable),
    }
}
/// Version 1 held the four starter games; version 2 added sixteen more.
fn first_collection(payload: &Value) -> Result<Value, Error> {
    let current = payload
        .get("current")
        .and_then(Value::as_i64)
        .ok_or(Error::Corrupt)?;
    if !(0..=4).contains(&current)
        || payload
            .get("current")
            .and_then(Value::as_f64)
            .ok_or(Error::Corrupt)?
            .fract()
            != 0.0
    {
        return Err(Error::Corrupt);
    }
    let old = payload
        .get("games")
        .and_then(Value::as_array)
        .ok_or(Error::Corrupt)?;
    if old.len() != 4 {
        return Err(Error::Corrupt);
    }
    let mut records = old.to_vec();
    for p in &collection::puzzles()[4..VERSION_TWO_PUZZLES] {
        records.push(record(&p.id, b"", &[]));
    }
    Ok(ObjectBuilder::new()
        .set(
            "current",
            if current == 0 {
                Value::Null
            } else {
                Value::String((current - 1).to_string())
            },
        )
        .set("games", Value::Array(records))
        .build())
}
/// Version 2 named the open puzzle by its place and listed every puzzle,
/// played or not.
fn by_id(payload: &Value) -> Result<Value, Error> {
    let current = match payload.get("current").ok_or(Error::Corrupt)? {
        Value::Null => Value::Null,
        value => {
            let index = value
                .as_str()
                .and_then(|i| i.parse::<usize>().ok())
                .filter(|i| *i < VERSION_TWO_PUZZLES)
                .ok_or(Error::Corrupt)?;
            Value::String(collection::puzzles()[index].id.clone())
        }
    };
    let unplayed = |record: &Value| {
        record.get("position").and_then(Value::as_str) == Some("")
            && record
                .get("undo")
                .and_then(Value::as_array)
                .is_some_and(<[Value]>::is_empty)
    };
    let records = payload
        .get("games")
        .and_then(Value::as_array)
        .ok_or(Error::Corrupt)?
        .iter()
        .filter(|record| !unplayed(record))
        .cloned()
        .collect();
    Ok(ObjectBuilder::new()
        .set("current", current)
        .set("games", Value::Array(records))
        .build())
}
pub fn decode(bytes: &[u8]) -> Option<(Vec<Run>, Option<usize>)> {
    if bytes.len() > LIMIT {
        return None;
    }
    if bytes.first().is_some_and(u8::is_ascii_digit) {
        let mut game = Game::default();
        if bytes.len() > 128 || !game.restore(bytes) {
            return None;
        }
        let mut games = vec![Run::default(); collection::puzzles().len()];
        let current = (game.kind != Kind::Home).then_some(game.puzzle);
        if let Some(i) = current {
            games[i].position = game.encode();
        }
        return Some((games, current));
    }
    let payload = schema()
        .restore(Some(bytes), |version, payload| migrate(version, &payload))
        .ok()??
        .payload;
    let index: BTreeMap<&str, usize> = collection::puzzles()
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    // A puzzle this build does not have is left out rather than refusing the
    // whole record, so a later collection that retires one still opens.
    let current = match payload.get("current")? {
        Value::Null => None,
        v => index.get(v.as_str()?).copied(),
    };
    let mut games = vec![Run::default(); collection::puzzles().len()];
    let mut seen = BTreeSet::new();
    for record in payload.get("games")?.as_array()? {
        let id = record.get("id")?.as_str()?;
        let position = record.get("position")?.as_str()?.as_bytes();
        let undo = record.get("undo")?.as_array()?;
        if !seen.insert(id) || undo.len() > HISTORY || position.is_empty() {
            return None;
        }
        let Some(&at) = index.get(id) else {
            continue;
        };
        games[at] = Run {
            position: canonical(position, at)?,
            undo: undo
                .iter()
                .map(|v| canonical(v.as_str()?.as_bytes(), at))
                .collect::<Option<Vec<_>>>()?,
        };
    }
    Some((games, current))
}

pub fn position_header(fields: &[&str]) -> Option<(Kind, usize, [u8; 64], u64)> {
    let (kind, index, legacy) = if let Some(raw) = fields[0].strip_prefix('p') {
        let Ok(index) = raw.parse::<usize>() else {
            return None;
        };
        let puzzle = collection::puzzles().get(index)?;
        (puzzle.kind, index, false)
    } else {
        let kind = fields[0].parse().ok().and_then(Kind::read)?;
        (kind, usize::from(kind.stored().saturating_sub(1)), true)
    };
    let cells = fields[1]
        .split(',')
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok();
    let values = cells?;
    if values.len() != if legacy { 16 } else { 64 } || values.iter().any(|c| *c > 9) {
        return None;
    }
    let mut cells = [0; 64];
    cells[..values.len()].copy_from_slice(&values);
    let Ok(mines) = fields[2].parse::<u64>() else {
        return None;
    };
    let puzzle = &collection::puzzles()[index];
    if kind == Kind::Mines
        && (mines.count_ones() != puzzle.mines().count_ones()
            || mines
                .checked_shr(u32::try_from(puzzle.side() * puzzle.side()).unwrap_or(64))
                .unwrap_or(0)
                != 0)
    {
        return None;
    }

    Some((kind, index, cells, mines))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written by the twenty-puzzle build: a Hashi with two moves, a solved
    /// Kakuro and a Minesweeper field left open.
    const VERSION_TWO: &str = r#"{"schema":"logicpack.games","version":2,"payload":{"current":"18","games":[{"id":"slither-four-twos","position":"","undo":[]},{"id":"hashi-cross","position":"p1|2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0","undo":["p1|0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0","p1|1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0"]},{"id":"kakuro-three-cells","position":"p2|3,2,4,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|1","undo":["p2|0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0","p2|3,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0","p2|3,2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0","p2|3,2,4,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0"]},{"id":"mines-four-square","position":"","undo":[]},{"id":"slither-01","position":"","undo":[]},{"id":"slither-02","position":"","undo":[]},{"id":"slither-03","position":"","undo":[]},{"id":"slither-04","position":"","undo":[]},{"id":"hashi-01","position":"","undo":[]},{"id":"hashi-02","position":"","undo":[]},{"id":"hashi-03","position":"","undo":[]},{"id":"hashi-04","position":"","undo":[]},{"id":"kakuro-01","position":"","undo":[]},{"id":"kakuro-02","position":"","undo":[]},{"id":"kakuro-03","position":"","undo":[]},{"id":"kakuro-04","position":"","undo":[]},{"id":"mines-01","position":"","undo":[]},{"id":"mines-02","position":"","undo":[]},{"id":"mines-03","position":"p18|0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|822157857|0|0|0|0","undo":["p18|0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|822157857|0|1|0|0"]},{"id":"mines-04","position":"","undo":[]}]}}"#;

    #[test]
    fn a_save_from_the_twenty_puzzle_build_opens_with_its_progress() {
        let (games, current) = decode(VERSION_TWO.as_bytes()).expect("version 2 opens");
        assert_eq!(current, Some(18), "the open Minesweeper field stays open");
        assert_eq!(games.len(), collection::puzzles().len());
        assert!(games[1].position.starts_with(b"p1|2,"));
        assert_eq!(games[1].undo.len(), 2);
        assert!(
            games[2].position.ends_with(b"|0|1"),
            "the Kakuro stays solved"
        );
        assert_eq!(games[2].undo.len(), 4);
        assert!(games[18].position.starts_with(b"p18|"));
        let others = games
            .iter()
            .enumerate()
            .filter(|(i, run)| ![1, 2, 18].contains(i) && **run != Run::default());
        assert_eq!(others.count(), 0);

        let again = encode(&games, current);
        assert!(String::from_utf8_lossy(&again).contains("\"version\":3"));
        assert_eq!(decode(&again), Some((games, current)));
    }

    #[test]
    fn records_are_matched_by_id_and_unknown_puzzles_are_left_out() {
        let mut games = vec![Run::default(); collection::puzzles().len()];
        let mut game = Game::default();
        game.select_puzzle(9);
        game.cells[0] = 2;
        games[9].position = game.encode();
        let bytes = String::from_utf8(encode(&games, Some(9))).unwrap();
        assert!(
            !bytes.contains("slither-four-twos"),
            "unplayed puzzles are not stored"
        );
        let retired = bytes.replace(
            "\"games\":[",
            "\"games\":[{\"id\":\"retired-puzzle\",\"position\":\"p0|0\",\"undo\":[]},",
        );
        let (decoded, current) = decode(retired.as_bytes()).expect("unknown ids are dropped");
        assert_eq!(current, Some(9));
        assert_eq!(decoded, games);
    }

    #[test]
    fn hundreds_of_puzzles_in_progress_still_fit_and_the_open_one_keeps_its_undo() {
        let mut games = Vec::new();
        let mut game = Game::default();
        for index in 0..collection::puzzles().len() {
            game.select_puzzle(index);
            game.cells[0] = 2;
            let position = game.encode();
            games.push(Run {
                undo: vec![position.clone(); HISTORY],
                position,
            });
        }
        let current = Some(collection::puzzles().len() - 1);
        let bytes = encode(&games, current);
        assert!(bytes.len() <= LIMIT, "{}", bytes.len());
        let (decoded, open) = decode(&bytes).expect("a trimmed record opens");
        assert_eq!(open, current);
        let last = decoded.len() - 1;
        assert_eq!(
            decoded[last], games[last],
            "the open puzzle keeps every step"
        );
        for (run, original) in decoded.iter().zip(&games) {
            assert_eq!(run.position, original.position, "no progress is lost");
        }
    }
}
