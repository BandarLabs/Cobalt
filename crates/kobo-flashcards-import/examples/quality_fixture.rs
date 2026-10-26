//! Generate an original, text-only study package for companion acceptance.
use anki::{collection::CollectionBuilder, decks::DeckId, storage::SchemaVersion};
use std::{fs, io::Write, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(std::env::args().nth(1).ok_or("expected output APKG path")?);
    if output.exists() {
        return Err("fixture output already exists".into());
    }
    let scratch = output.with_extension("fixture.anki2");
    if scratch.exists() {
        return Err("fixture collection already exists".into());
    }
    let mut collection = CollectionBuilder::new(&scratch).build()?;
    let basic = collection
        .get_notetype_by_name("Basic")?
        .ok_or("Basic notetype missing")?;
    for (question, answer) in [
        ("What does a compass point toward?", "Magnetic north."),
        (
            "What is the name for water turning into vapour?",
            "Evaporation.",
        ),
        ("How many sides does a hexagon have?", "Six."),
    ] {
        let mut note = basic.new_note();
        note.set_field(0, question)?;
        note.set_field(1, answer)?;
        collection.add_note(&mut note, DeckId(1))?;
    }
    collection.close(Some(SchemaVersion::V11))?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("collection.anki2", options)?;
    archive.write_all(&fs::read(&scratch)?)?;
    archive.start_file("media", options)?;
    archive.write_all(b"{}")?;
    archive.finish()?;
    fs::remove_file(scratch)?;
    println!("Created three original study cards at {}", output.display());
    Ok(())
}
