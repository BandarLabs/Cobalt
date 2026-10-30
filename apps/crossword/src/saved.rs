use crate::game::{Position, Progress, HISTORY, PUZZLES};
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};
pub const KEY: &str = "crossword-state-v1";
/// Room for a game of every puzzle at once, with the current one's undo steps.
///
/// Version 1 wrote a record for every puzzle on every save, which was fine
/// for four puzzles and would be several hundred kilobytes for seventy, all
/// rewritten on every letter. Version 2 writes only the puzzles that have
/// been played.
pub const LIMIT: usize = 64 * 1024;
fn schema() -> Schema {
    Schema::new("crossword.games", 2, LIMIT).expect("fixed schema")
}
fn position(p: &Position) -> Value {
    ObjectBuilder::new()
        .set(
            "letters",
            String::from_utf8(p.letters.clone()).expect("ASCII board"),
        )
        .set("selected", p.selected.to_string())
        .set("down", p.down)
        .build()
}
fn record(g: &Progress, p: &crate::game::Puzzle, undo: bool) -> Value {
    let steps = if undo {
        g.undo.iter().map(position).collect()
    } else {
        Vec::new()
    };
    ObjectBuilder::new()
        .set("id", p.id)
        .set(
            "answer",
            std::str::from_utf8(p.answer).expect("ASCII answer"),
        )
        .set("position", position(&g.position))
        .set("undo", Value::Array(steps))
        .set("reveals", g.reveals)
        .set("checks", g.checks)
        .set("solved", g.solved_once)
        .build()
}
/// The record for every puzzle that has been touched.
///
/// A puzzle nobody has opened is left out, since it decodes to exactly what a
/// fresh one is. When the undo steps of every puzzle together would not fit,
/// only the current puzzle keeps them: the steps of a puzzle you are not
/// solving are the least useful thing in the record, and the letters must
/// always fit.
pub fn encode(games: &[Progress], current: usize) -> Vec<u8> {
    let played: Vec<(usize, &Progress)> = games
        .iter()
        .enumerate()
        .filter(|(index, g)| **g != Progress::new(&PUZZLES[*index]))
        .collect();
    let build = |all_undo: bool| {
        let records = played
            .iter()
            .map(|(index, g)| record(g, &PUZZLES[*index], all_undo || *index == current))
            .collect();
        schema().encode(
            &ObjectBuilder::new()
                .set("current", current.to_string())
                .set("games", Value::Array(records))
                .build(),
        )
    };
    build(true)
        .or_else(|_| build(false))
        .expect("bounded games")
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v.get(key).and_then(Value::as_str).ok_or(Error::Corrupt)
}
fn flag(v: &Value, key: &str) -> Result<bool, Error> {
    v.get(key).and_then(Value::as_bool).ok_or(Error::Corrupt)
}
fn number(v: &Value, key: &str) -> Result<u32, Error> {
    let raw = v.get(key).and_then(Value::as_f64).ok_or(Error::Corrupt)?;
    if raw.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&raw) {
        return Err(Error::Corrupt);
    }
    v.get(key)
        .and_then(Value::as_i64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(Error::Corrupt)
}
fn read_position(v: &Value, answer: &[u8]) -> Result<Position, Error> {
    let letters = text(v, "letters")?.as_bytes().to_vec();
    let selected = text(v, "selected")?
        .parse::<usize>()
        .map_err(|_| Error::Corrupt)?;
    if letters.len() != answer.len()
        || !letters.iter().zip(answer).all(|(b, a)| {
            if *a == b'#' {
                *b == b'#'
            } else {
                *b == b'.' || b.is_ascii_uppercase()
            }
        })
        || selected >= answer.len()
        || answer[selected] == b'#'
    {
        return Err(Error::Corrupt);
    }
    Ok(Position {
        letters,
        selected,
        down: flag(v, "down")?,
    })
}
pub fn decode(bytes: &[u8]) -> Result<(Vec<Progress>, usize), Error> {
    if bytes.len() > LIMIT {
        return Err(Error::TooLarge);
    }
    // The shipped 5×5 save remains at this key. Migration is read-only until the next edit.
    if !bytes.starts_with(b"{") {
        return legacy(bytes);
    }
    // Version 1 listed all four puzzles of its day in order, which version 2
    // reads as it stands: a list of played puzzles that happens to name every
    // one of them.
    let v = schema()
        .restore(Some(bytes), |version, payload| {
            if version == 1 {
                Ok(payload)
            } else {
                Err(Error::MigrationUnavailable)
            }
        })?
        .ok_or(Error::Corrupt)?
        .payload;
    let current = text(&v, "current")?
        .parse::<usize>()
        .map_err(|_| Error::Corrupt)?;
    let records = v
        .get("games")
        .and_then(Value::as_array)
        .ok_or(Error::Corrupt)?;
    if current >= PUZZLES.len() || records.len() > PUZZLES.len() {
        return Err(Error::Corrupt);
    }
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    let mut seen = vec![false; PUZZLES.len()];
    for v in records {
        // A puzzle this version does not know, or one whose grid changed
        // under it, makes the whole record unreadable rather than quietly
        // dropping someone's letters.
        let id = text(v, "id")?;
        let index = PUZZLES
            .iter()
            .position(|p| p.id == id)
            .ok_or(Error::Corrupt)?;
        let p = &PUZZLES[index];
        if std::mem::replace(&mut seen[index], true) || text(v, "answer")?.as_bytes() != p.answer {
            return Err(Error::Corrupt);
        }
        let steps = v
            .get("undo")
            .and_then(Value::as_array)
            .ok_or(Error::Corrupt)?;
        if steps.len() > HISTORY {
            return Err(Error::Corrupt);
        }
        let g = Progress {
            position: read_position(v.get("position").ok_or(Error::Corrupt)?, p.answer)?,
            undo: steps
                .iter()
                .map(|s| read_position(s, p.answer))
                .collect::<Result<_, _>>()?,
            checks: number(v, "checks")?,
            reveals: number(v, "reveals")?,
            solved_once: flag(v, "solved")?,
        };
        if g.solved(p) && !g.solved_once {
            return Err(Error::Corrupt);
        }
        games[index] = g;
    }
    Ok((games, current))
}
fn legacy(bytes: &[u8]) -> Result<(Vec<Progress>, usize), Error> {
    let s = std::str::from_utf8(bytes).map_err(|_| Error::Corrupt)?;
    let fields = s.split(';').collect::<Vec<_>>();
    let [letters, selected, direction] = fields.as_slice() else {
        return Err(Error::Corrupt);
    };
    let v = ObjectBuilder::new()
        .set("letters", *letters)
        .set("selected", if *selected == "-" { "0" } else { *selected })
        .set(
            "down",
            match *direction {
                "0" => false,
                "1" => true,
                _ => return Err(Error::Corrupt),
            },
        )
        .build();
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    games[2].position = read_position(&v, PUZZLES[2].answer)?;
    games[2].solved_once = games[2].solved(&PUZZLES[2]);
    Ok((games, 2))
}
