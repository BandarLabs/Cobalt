//! Export an original study deck the way current Anki does by default: a
//! `meta` member, a zstd-compressed `collection.anki21b` and a protobuf media
//! list. Pass `--older` for the layout Anki writes when "Support older Anki
//! versions" is ticked. Used to check the importer against real exports.
use anki::collection::CollectionBuilder;
use anki::decks::DeckId;
use anki::import_export::package::ExportAnkiPackageOptions;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let output = PathBuf::from(arguments.next().ok_or("expected output APKG path")?);
    let legacy = arguments.next().as_deref() == Some("--older");
    if output.exists() {
        return Err("export output already exists".into());
    }
    let scratch = output.with_extension("export.anki2");
    if scratch.exists() {
        return Err("export collection already exists".into());
    }
    let mut collection = CollectionBuilder::new(&scratch).build()?;
    let basic = collection
        .get_notetype_by_name("Basic")?
        .ok_or("Basic notetype missing")?;
    for (question, answer) in [
        ("What does a compass point toward?", "Magnetic north."),
        ("What is water turning into vapour called?", "Evaporation."),
        ("How many sides does a hexagon have?", "Six."),
        ("Which planet is closest to the Sun?", "Mercury."),
        ("What is the largest ocean?", "The Pacific."),
        ("How many minutes are in a day?", "1,440."),
        ("What gas do plants take in?", "Carbon dioxide."),
        ("What is the freezing point of water in Celsius?", "Zero degrees."),
    ] {
        let mut note = basic.new_note();
        note.set_field(0, question)?;
        note.set_field(1, answer)?;
        collection.add_note(&mut note, DeckId(1))?;
    }
    collection.export_apkg(
        &output,
        ExportAnkiPackageOptions {
            with_scheduling: true,
            with_deck_configs: true,
            with_media: true,
            legacy,
        },
        "",
        None,
    )?;
    collection.close(None)?;
    std::fs::remove_file(&scratch)?;
    println!(
        "Exported eight original study cards to {} ({} layout)",
        output.display(),
        if legacy { "older" } else { "current" }
    );
    Ok(())
}
