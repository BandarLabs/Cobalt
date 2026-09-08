//! Bounded progress and undo for the four original puzzles.
use crate::{Game, Kind};
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::Schema;

pub const LIMIT: usize = 24 * 1024;
pub const HISTORY: usize = 32;
const IDS: [&str; 4] = [
    "slither-four-twos",
    "hashi-cross",
    "kakuro-three-cells",
    "mines-four-square",
];

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct Run {
    pub position: Vec<u8>,
    pub undo: Vec<Vec<u8>>,
}
fn schema() -> Schema {
    Schema::new("logicpack.games", 1, LIMIT).expect("fixed schema")
}
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("ASCII position")
}
pub fn encode(games: &[Run; 4], current: Kind) -> Vec<u8> {
    schema()
        .encode(
            &ObjectBuilder::new()
                .set("current", u32::from(current.stored()))
                .set(
                    "games",
                    Value::Array(
                        games
                            .iter()
                            .zip(IDS)
                            .map(|(run, id)| {
                                ObjectBuilder::new()
                                    .set("id", id)
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
        .expect("bounded games")
}
fn valid(bytes: &[u8], kind: Kind) -> bool {
    if bytes.len() > 128 {
        return false;
    }
    let mut game = Game::default();
    game.restore(bytes) && game.kind == kind
}
pub fn decode(bytes: &[u8]) -> Option<([Run; 4], Kind)> {
    if bytes.len() > LIMIT {
        return None;
    }
    // Existing records contain one game. Preserve it until the next acknowledged edit.
    if bytes.first().is_some_and(u8::is_ascii_digit) {
        let mut game = Game::default();
        if bytes.len() > 128 || !game.restore(bytes) {
            return None;
        }
        let mut games = std::array::from_fn(|_| Run::default());
        if game.kind != Kind::Home {
            games[usize::from(game.kind.stored() - 1)].position = bytes.to_vec();
        }
        return Some((games, game.kind));
    }
    let v = schema()
        .restore(Some(bytes), |_, _| {
            Err(kobo_state::record::Error::MigrationUnavailable)
        })
        .ok()??
        .payload;
    let current = v.get("current")?.as_i64()?;
    if v.get("current")?.as_f64()?.fract() != 0.0 {
        return None;
    }
    let current = Kind::read(u8::try_from(current).ok()?)?;
    let records = v.get("games")?.as_array()?;
    if records.len() != 4 {
        return None;
    }
    let mut games = std::array::from_fn(|_| Run::default());
    for (index, record) in records.iter().enumerate() {
        if record.get("id")?.as_str()? != IDS[index] {
            return None;
        }
        let kind = Kind::read(u8::try_from(index + 1).ok()?)?;
        let position = record.get("position")?.as_str()?.as_bytes();
        let undo = record.get("undo")?.as_array()?;
        if undo.len() > HISTORY
            || (position.is_empty() && !undo.is_empty())
            || (!position.is_empty() && !valid(position, kind))
        {
            return None;
        }
        let history = undo
            .iter()
            .map(|v| {
                let bytes = v.as_str()?.as_bytes();
                valid(bytes, kind).then(|| bytes.to_vec())
            })
            .collect::<Option<Vec<_>>>()?;
        games[index] = Run {
            position: position.to_vec(),
            undo: history,
        };
    }
    if current != Kind::Home && games[usize::from(current.stored() - 1)].position.is_empty() {
        return None;
    }
    Some((games, current))
}
