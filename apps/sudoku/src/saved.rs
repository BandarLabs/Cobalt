//! One bounded record keeps puzzle identity, marks, settings and 64 undo steps.
use super::game::{digit_text, digits, Game, Position, Puzzle, CELLS, HISTORY};
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};
use std::collections::VecDeque;
use std::fmt::Write;
pub const KEY: &str = "game";
pub const LIMIT: usize = 48 * 1024;
fn schema() -> Schema {
    Schema::new("sudoku.game", 1, LIMIT).expect("fixed schema")
}
fn position_value(position: &Position) -> Value {
    let mut notes = String::new();
    for note in position.notes {
        write!(notes, "{note:03x}").expect("string write");
    }
    ObjectBuilder::new()
        .set("board", digit_text(&position.board))
        .set("notes", notes)
        .set(
            "selected",
            position
                .selected
                .map_or_else(|| "none".into(), |cell| cell.to_string()),
        )
        .set("hints", u32::from(position.hints))
        .build()
}
pub fn encode(game: &Game, puzzles: &[Puzzle]) -> Result<Vec<u8>, Error> {
    schema().encode(
        &ObjectBuilder::new()
            .set("puzzle", game.puzzle.to_string())
            .set("clues", digit_text(&puzzles[game.puzzle].clues))
            .set("position", position_value(&game.position))
            .set("pencil", game.pencil)
            .set("checking", game.checking)
            .set("landscape", game.landscape)
            .set(
                "undo",
                Value::Array(game.undo.iter().map(position_value).collect()),
            )
            .build(),
    )
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v.get(key).and_then(Value::as_str).ok_or(Error::Corrupt)
}
fn position(v: &Value, puzzle: &Puzzle) -> Result<Position, Error> {
    let board = digits(text(v, "board")?).ok_or(Error::Corrupt)?;
    let raw = text(v, "notes")?;
    if raw.len() != CELLS * 3 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Corrupt);
    }
    let mut notes = [0; CELLS];
    for (cell, note) in notes.iter_mut().enumerate() {
        *note =
            u16::from_str_radix(&raw[cell * 3..cell * 3 + 3], 16).map_err(|_| Error::Corrupt)?;
        if *note > 511
            || (board[cell] != 0 && *note != 0)
            || (puzzle.clues[cell] != 0 && board[cell] != puzzle.clues[cell])
        {
            return Err(Error::Corrupt);
        }
    }
    let selected = match text(v, "selected")? {
        "none" => None,
        s => Some(s.parse::<usize>().map_err(|_| Error::Corrupt)?),
    };
    if selected.is_some_and(|n| n >= CELLS) {
        return Err(Error::Corrupt);
    }
    let hints = v
        .get("hints")
        .and_then(Value::as_f64)
        .ok_or(Error::Corrupt)?;
    if hints.fract() != 0.0 || !(0.0..=f64::from(u16::MAX)).contains(&hints) {
        return Err(Error::Corrupt);
    }
    let hints = v
        .get("hints")
        .and_then(Value::as_i64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or(Error::Corrupt)?;
    Ok(Position {
        board,
        notes,
        selected,
        hints,
    })
}
pub fn decode(bytes: &[u8], puzzles: &[Puzzle]) -> Result<Game, Error> {
    let value = schema()
        .restore(Some(bytes), |_, _| Err(Error::MigrationUnavailable))?
        .ok_or(Error::Corrupt)?
        .payload;
    let puzzle = text(&value, "puzzle")?
        .parse::<usize>()
        .map_err(|_| Error::Corrupt)?;
    let spec = puzzles.get(puzzle).ok_or(Error::Corrupt)?;
    if text(&value, "clues")? != digit_text(&spec.clues) {
        return Err(Error::Corrupt);
    }
    let current = position(value.get("position").ok_or(Error::Corrupt)?, spec)?;
    let steps = value
        .get("undo")
        .and_then(Value::as_array)
        .ok_or(Error::Corrupt)?;
    if steps.len() > HISTORY {
        return Err(Error::Corrupt);
    }
    let undo = steps
        .iter()
        .map(|v| position(v, spec))
        .collect::<Result<VecDeque<_>, _>>()?;
    let flag = |key| {
        value
            .get(key)
            .and_then(Value::as_bool)
            .ok_or(Error::Corrupt)
    };
    Ok(Game {
        puzzle,
        position: current,
        undo,
        pencil: flag("pencil")?,
        checking: flag("checking")?,
        landscape: flag("landscape")?,
    })
}
