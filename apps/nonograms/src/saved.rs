//! Exact puzzle identity and bounded persistent move history.
use super::{Game, Mark};
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};
pub const LIMIT: usize = 64 * 1024;
fn schema() -> Schema {
    Schema::new("nonograms.game", 1, LIMIT).expect("fixed schema")
}
fn marks(values: &[Mark]) -> String {
    values
        .iter()
        .map(|mark| char::from(mark.stored()))
        .collect()
}
fn identity(game: &Game) -> String {
    let puzzle = game.puzzle().expect("active puzzle");
    kobo_net::sha256::hex_digest(
        &puzzle
            .answer
            .iter()
            .map(|filled| u8::from(*filled))
            .collect::<Vec<_>>(),
    )
}
pub fn encode(game: &Game) -> Result<Vec<u8>, Error> {
    schema().encode(
        &ObjectBuilder::new()
            .set("identity", identity(game))
            .set("marks", marks(&game.marks))
            .set("guided", game.guided)
            .set("run_entry", game.run_entry)
            .set(
                "focus",
                game.focus
                    .map_or_else(|| "none".into(), |cell| cell.to_string()),
            )
            .set(
                "undo",
                Value::Array(
                    game.undo
                        .iter()
                        .map(|step| Value::String(marks(step)))
                        .collect(),
                ),
            )
            .build(),
    )
}
fn read_marks(value: &str, count: usize) -> Result<Vec<Mark>, Error> {
    if value.len() != count {
        return Err(Error::Corrupt);
    }
    value
        .bytes()
        .map(Mark::read)
        .collect::<Option<_>>()
        .ok_or(Error::Corrupt)
}
pub fn restore(game: &mut Game, bytes: &[u8]) -> Result<(), Error> {
    let count = game.marks.len();
    // The shipped per-puzzle record contains just a mode and marks. Preserve
    // its key, migrate in memory, and write the new schema on the next edit.
    if bytes.len() == count + 2 && matches!(bytes[0], b'f' | b'g') && bytes[1] == b'\n' {
        let text = std::str::from_utf8(&bytes[2..]).map_err(|_| Error::Corrupt)?;
        let restored = read_marks(text, count)?;
        game.marks = restored;
        game.guided = bytes[0] == b'g';
        game.undo.clear();
        game.done = game.completed();
        return Ok(());
    }
    let value = schema()
        .restore(Some(bytes), |_, _| Err(Error::MigrationUnavailable))?
        .ok_or(Error::Corrupt)?
        .payload;
    let text = |key| value.get(key).and_then(Value::as_str).ok_or(Error::Corrupt);
    if text("identity")? != identity(game) {
        return Err(Error::Corrupt);
    }
    let restored = read_marks(text("marks")?, count)?;
    let steps = value
        .get("undo")
        .and_then(Value::as_array)
        .ok_or(Error::Corrupt)?;
    if steps.len() > 64 {
        return Err(Error::Corrupt);
    }
    let undo = steps
        .iter()
        .map(|value| read_marks(value.as_str().ok_or(Error::Corrupt)?, count))
        .collect::<Result<_, _>>()?;
    let focus = match text("focus")? {
        "none" => None,
        s => Some(s.parse::<usize>().map_err(|_| Error::Corrupt)?),
    };
    if focus.is_some_and(|cell| cell >= count) {
        return Err(Error::Corrupt);
    }
    let flag = |key| {
        value
            .get(key)
            .and_then(Value::as_bool)
            .ok_or(Error::Corrupt)
    };
    let guided = flag("guided")?;
    let run_entry = flag("run_entry")?;
    game.marks = restored;
    game.undo = undo;
    game.focus = focus;
    game.guided = guided;
    game.run_entry = run_entry;
    game.done = game.completed();
    Ok(())
}
