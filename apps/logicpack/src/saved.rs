//! Versioned progress and undo for stable original collection identities.
use crate::{collection, Game, Kind};
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};
pub const LIMIT: usize = 128 * 1024;
pub const HISTORY: usize = 32;
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct Run {
    pub position: Vec<u8>,
    pub undo: Vec<Vec<u8>>,
}
fn schema() -> Schema {
    Schema::new("logicpack.games", 2, LIMIT).expect("fixed schema")
}
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("ASCII snapshot")
}
pub fn encode(games: &[Run], current: Option<usize>) -> Vec<u8> {
    schema()
        .encode(
            &ObjectBuilder::new()
                .set(
                    "current",
                    current.map_or(Value::Null, |i| Value::String(i.to_string())),
                )
                .set(
                    "games",
                    Value::Array(
                        games
                            .iter()
                            .zip(collection::puzzles())
                            .map(|(run, p)| {
                                ObjectBuilder::new()
                                    .set("id", p.id.clone())
                                    .set("position", text(&run.position))
                                    .set(
                                        "undo",
                                        Value::Array(
                                            run.undo
                                                .iter()
                                                .map(|p| Value::String(text(p).into()))
                                                .collect(),
                                        ),
                                    )
                                    .build()
                            })
                            .collect(),
                    ),
                )
                .build(),
        )
        .expect("bounded collection")
}
fn canonical(bytes: &[u8], index: usize) -> Option<Vec<u8>> {
    if bytes.len() > 192 {
        return None;
    }
    let mut game = Game::default();
    (game.restore(bytes) && game.kind != Kind::Home && game.puzzle == index).then(|| game.encode())
}
fn migrate(version: u32, payload: &Value) -> Result<Value, Error> {
    if version != 1 {
        return Err(Error::MigrationUnavailable);
    }
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
    for p in &collection::puzzles()[4..] {
        records.push(
            ObjectBuilder::new()
                .set("id", p.id.clone())
                .set("position", "")
                .set("undo", Value::Array(vec![]))
                .build(),
        );
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
    let current = match payload.get("current")? {
        Value::Null => None,
        v => Some(v.as_str()?.parse::<usize>().ok()?),
    };
    let records = payload.get("games")?.as_array()?;
    if records.len() != collection::puzzles().len() {
        return None;
    }
    let mut games = Vec::new();
    for (index, (record, p)) in records.iter().zip(collection::puzzles()).enumerate() {
        if record.get("id")?.as_str()? != p.id {
            return None;
        }
        let position = record.get("position")?.as_str()?.as_bytes();
        let undo = record.get("undo")?.as_array()?;
        if undo.len() > HISTORY || position.is_empty() && !undo.is_empty() {
            return None;
        }
        let position = if position.is_empty() {
            vec![]
        } else {
            canonical(position, index)?
        };
        let undo = undo
            .iter()
            .map(|v| canonical(v.as_str()?.as_bytes(), index))
            .collect::<Option<Vec<_>>>()?;
        games.push(Run { position, undo });
    }
    if current.is_some_and(|i| games.get(i).is_none_or(|run| run.position.is_empty())) {
        return None;
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
            || mines >> (puzzle.side() * puzzle.side()) != 0)
    {
        return None;
    }

    Some((kind, index, cells, mines))
}
