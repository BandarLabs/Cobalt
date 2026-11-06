//! Turns Tabler's SVGs into the Rust this project draws icons from.
//!
//! # Why generate rather than parse at runtime
//!
//! An icon set is artwork, and artwork belongs in the repository where it can
//! be reviewed, diffed and rendered without a network. This program runs by
//! hand, writes one Rust file, and is then irrelevant: nothing it produces
//! depends on it, and the workspace keeps its no-dependency rule intact.
//!
//! # Usage
//!
//! ```text
//! cargo run -p icon-import -- <tabler-icons-checkout> [output.rs]
//! ```
//!
//! `scripts/import-icons.sh` fetches the checkout, verifies it and calls this.

mod svg;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use svg::Step;

/// The side of the box Tabler designs in.
const SOURCE_UNITS: f64 = 24.0;
/// The side of the box this project designs in, from `kobo_ui::vector::UNITS`.
const TARGET_UNITS: f64 = 1000.0;

/// Which glyph is drawn by which Tabler icon.
struct Entry {
    glyph: String,
    /// A Tabler icon name, or `=` and another glyph whose artwork this one
    /// borrows.
    icon: String,
}

impl Entry {
    /// The glyph whose artwork this one is drawn with, when it has none of
    /// its own.
    ///
    /// A glyph that keeps its name and wire number but changes its picture to
    /// one the set already has would otherwise carry a second copy of the
    /// same outline, and the two copies could drift apart.
    fn drawn_as(&self) -> Option<&str> {
        self.icon.strip_prefix('=')
    }
}

/// The glyphs drawn by hand in the renderer rather than from an outline.
///
/// Board pieces are discs and rings rather than line art, the two keyboard
/// keys keep geometry of their own, and the puzzle marks are cells and
/// pieces Tabler does not draw, so none of them has a Tabler icon behind it.
const HAND_DRAWN: [&str; 17] = [
    "BlackDisc",
    "WhiteDisc",
    "BlackDraughtsKing",
    "WhiteDraughtsKing",
    "BlackDraughtsMan",
    "WhiteDraughtsMan",
    "BoardPoint",
    "Mill",
    "LegalPoint",
    "Backspace",
    "Shift",
    "CrosswordGrid",
    "CellGrid",
    "NoughtsCrosses",
    "CluedGrid",
    "ComicPanels",
    "BoardPieces",
];

fn main() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let source = arguments
        .first()
        .ok_or("usage: icon-import <tabler-icons-checkout> [output.rs]")?;
    let default_output = "crates/kobo-ui/src/vector/tabler.rs".to_owned();
    let output = arguments.get(1).unwrap_or(&default_output);

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons.txt");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|error| format!("read {}: {error}", manifest.display()))?;
    let entries = read_manifest(&text)?;

    let mut rendered = String::new();
    for entry in &entries {
        if entry.drawn_as().is_some() {
            continue;
        }
        let file = Path::new(source)
            .join("icons")
            .join("outline")
            .join(format!("{}.svg", entry.icon));
        let document = std::fs::read_to_string(&file)
            .map_err(|error| format!("read {}: {error}", file.display()))?;
        let outlines =
            outlines_of(&document).map_err(|error| format!("{}: {error}", file.display()))?;
        if outlines.is_empty() {
            return Err(format!("{} has no paths", file.display()));
        }
        write_glyph(&mut rendered, entry, &outlines);
    }

    let file = render(&entries, &rendered);
    let target = PathBuf::from(output);
    std::fs::write(&target, file)
        .map_err(|error| format!("write {}: {error}", target.display()))?;
    println!("{} glyphs written to {}", entries.len(), target.display());
    Ok(())
}

/// Reads the glyph-to-icon table, rejecting anything ambiguous.
///
/// # Errors
///
/// When a line is not two words, or when a glyph or an icon appears twice.
/// A duplicate would silently win or lose depending on where it sat, and the
/// result would be one wrong icon in a set of dozens.
fn read_manifest(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    let mut glyphs = BTreeSet::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(glyph), Some(icon), None) = (words.next(), words.next(), words.next()) else {
            return Err(format!("line {} is not 'Glyph icon-name'", number + 1));
        };
        if !glyphs.insert(glyph.to_owned()) {
            return Err(format!("{glyph} is named twice"));
        }
        entries.push(Entry {
            glyph: glyph.to_owned(),
            icon: icon.to_owned(),
        });
    }
    for entry in &entries {
        if let Some(other) = entry.drawn_as() {
            let borrowed = entries
                .iter()
                .find(|candidate| candidate.glyph == other)
                .ok_or_else(|| {
                    format!("{} is drawn as {other}, which is not listed", entry.glyph)
                })?;
            if borrowed.drawn_as().is_some() {
                return Err(format!(
                    "{} is drawn as {other}, which is itself drawn as another glyph",
                    entry.glyph
                ));
            }
        }
    }
    Ok(entries)
}

/// Every outline of one document, scaled into the target box.
///
/// # Errors
///
/// When the document or any of its path data is malformed.
fn outlines_of(document: &str) -> Result<Vec<Vec<Step>>, String> {
    let scale = TARGET_UNITS / SOURCE_UNITS;
    let mut outlines = Vec::new();
    for data in svg::path_data(document)? {
        let steps = svg::parse(&data)?;
        outlines.push(steps.into_iter().map(|step| scaled(step, scale)).collect());
    }
    Ok(outlines)
}

fn scaled(step: Step, scale: f64) -> Step {
    match step {
        Step::Move(x, y) => Step::Move(x * scale, y * scale),
        Step::Line(x, y) => Step::Line(x * scale, y * scale),
        Step::Cubic(ax, ay, bx, by, x, y) => Step::Cubic(
            ax * scale,
            ay * scale,
            bx * scale,
            by * scale,
            x * scale,
            y * scale,
        ),
        Step::Close => Step::Close,
    }
}

fn round(value: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        value.round() as i32
    }
}

/// One `static` per glyph, named after it.
fn write_glyph(out: &mut String, entry: &Entry, outlines: &[Vec<Step>]) {
    let name = shout(&entry.glyph);
    let _ = writeln!(out, "/// `{}` from Tabler Icons.", entry.icon);
    let _ = writeln!(out, "static {name}: &[&[Cmd]] = &[");
    for outline in outlines {
        let _ = writeln!(out, "    &[");
        for step in outline {
            let line = match *step {
                Step::Move(x, y) => format!("Cmd::Move({}, {})", round(x), round(y)),
                Step::Line(x, y) => format!("Cmd::Line({}, {})", round(x), round(y)),
                Step::Cubic(ax, ay, bx, by, x, y) => format!(
                    "Cmd::Cubic({}, {}, {}, {}, {}, {})",
                    round(ax),
                    round(ay),
                    round(bx),
                    round(by),
                    round(x),
                    round(y)
                ),
                Step::Close => "Cmd::Close".to_owned(),
            };
            let _ = writeln!(out, "        {line},");
        }
        let _ = writeln!(out, "    ],");
    }
    let _ = writeln!(out, "];\n");
}

/// `MoreVertical` as `MORE_VERTICAL`.
fn shout(name: &str) -> String {
    let mut out = String::new();
    for (index, character) in name.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(character.to_ascii_uppercase());
    }
    out
}

/// The whole generated file.
fn render(entries: &[Entry], glyphs: &str) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    let _ = writeln!(
        out,
        "pub(super) fn outline(glyph: Glyph) -> &'static [&'static [Cmd]] {{"
    );
    let _ = writeln!(out, "    match glyph {{");
    for entry in entries {
        let artwork = match entry.drawn_as() {
            Some(other) => {
                let _ = writeln!(
                    out,
                    "        // Drawn with the artwork of `{other}`. See `Glyph::{}`.",
                    entry.glyph
                );
                shout(other)
            }
            None => shout(&entry.glyph),
        };
        let _ = writeln!(out, "        Glyph::{} => {artwork},", entry.glyph);
    }
    out.push_str(
        "        // Board pieces, the keyboard keys and the puzzle grids are drawn in\n\
         // `vector::shapes` rather than imported, for the reasons on `HAND_DRAWN`.\n",
    );
    for (index, glyph) in HAND_DRAWN.iter().enumerate() {
        let lead = if index == 0 { "" } else { "| " };
        let _ = writeln!(out, "        {lead}Glyph::{glyph}");
    }
    out.push_str("        => &[],\n");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "}}\n");
    out.push_str(glyphs);
    out
}

const HEADER: &str = "\
//! Icon geometry taken from Tabler Icons.
//!
//! Generated by `tools/icon-import`. Do not edit: run
//! `scripts/import-icons.sh` instead, and change `tools/icon-import/icons.txt`
//! to change which icon stands behind which glyph.
//!
//! Tabler Icons is Copyright (c) 2020-2024 Paweł Kuna and is offered under the
//! MIT licence, whose terms are reproduced in `licenses/LICENSE-Tabler.txt`.
//! The MIT licence imposes no copyleft, so this geometry travels inside an
//! AGPL binary without changing anything about it, as long as the notice
//! travels with it.
//!
//! Coordinates are the source artwork's 24 unit box scaled to the 1000 unit
//! box `super::UNITS` defines, so 1 source pixel is 41 and two thirds units.
//! Curves are cubics because that is what the source uses; quadratics and
//! elliptical arcs were resolved into cubics at import time, in `f64`, before
//! anything was rounded.

use super::Cmd;
use crate::Glyph;

/// Every outline of one glyph.
///
/// Deliberately total rather than fallible. A glyph with no artwork would draw
/// nothing at all, and nothing at all is invisible rather than wrong, so the
/// compiler is made to refuse a `Glyph` variant that `icons.txt` does not
/// name.
";

#[cfg(test)]
mod tests {
    use super::{outlines_of, read_manifest, render, shout, Entry, Step};

    /// The file this tool writes, as it stands in the repository.
    const CHECKED_IN: &str = include_str!("../../../crates/kobo-ui/src/vector/tabler.rs");

    #[test]
    fn every_hand_drawn_glyph_in_the_checked_in_file_is_one_this_tool_still_writes() {
        // The glyphs with no Tabler outline are named in a string here and
        // drawn by hand in the renderer, so the two lists drift apart in
        // silence: a glyph added to the renderer and not to this one turns the
        // next regeneration into a match that does not compile, which is found
        // by whoever runs the script rather than by whoever caused it.
        let arm = CHECKED_IN
            .split_once("=> &[],")
            .expect("the checked-in file has a fallback arm")
            .0;
        let hand_drawn: Vec<&str> = arm
            .rsplit("for the reasons on `HAND_DRAWN`.")
            .next()
            .expect("the fallback arm follows its comment")
            .split("Glyph::")
            .skip(1)
            .map(|name| {
                name.trim_end_matches(|character: char| !character.is_alphanumeric())
                    .trim()
            })
            .collect();
        assert!(
            hand_drawn.len() >= 2,
            "read no hand-drawn glyphs from the checked-in file"
        );
        let written = render(&[], "");
        for glyph in hand_drawn {
            assert!(
                written.contains(&format!("Glyph::{glyph}")),
                "{glyph} is drawn by hand in the renderer and this tool would not write it"
            );
        }
    }

    #[test]
    fn a_manifest_line_is_a_glyph_and_an_icon() {
        let entries = read_manifest("# a note\n\nTrash trash\nClose x\n").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].glyph, "Trash");
        assert_eq!(entries[1].icon, "x");
    }

    #[test]
    fn a_glyph_can_borrow_the_artwork_of_another_listed_glyph() {
        let entries = read_manifest("Headphones headphones\nBluetooth =Headphones\n").unwrap();
        let written = render(&entries, "");
        assert!(
            written.contains("Glyph::Bluetooth => HEADPHONES,"),
            "{written}"
        );
        assert!(!written.contains("BLUETOOTH"), "{written}");
    }

    #[test]
    fn borrowing_the_artwork_of_a_glyph_that_is_not_listed_is_refused() {
        assert!(read_manifest("Bluetooth =Headphones\n").is_err());
        assert!(read_manifest("A a\nB =A\nC =B\n").is_err());
    }

    #[test]
    fn naming_one_glyph_twice_is_refused() {
        assert!(read_manifest("Trash trash\nTrash bin\n").is_err());
    }

    #[test]
    fn a_line_that_is_not_two_words_is_refused() {
        assert!(read_manifest("Trash\n").is_err());
        assert!(read_manifest("Trash trash extra\n").is_err());
    }

    #[test]
    fn a_camel_case_glyph_becomes_a_shouting_constant() {
        assert_eq!(shout("Trash"), "TRASH");
        assert_eq!(shout("MoreVertical"), "MORE_VERTICAL");
        assert_eq!(shout("Rewind30"), "REWIND30");
    }

    #[test]
    fn the_source_box_is_scaled_to_the_target_box() {
        // 24 units in is 1000 units out, so the far corner lands on the far
        // corner and nothing needs a margin adding to it.
        let outlines = outlines_of("<path d=\"M0 0 L24 24\" />").unwrap();
        assert_eq!(outlines[0][0], Step::Move(0.0, 0.0));
        let Step::Line(x, y) = outlines[0][1] else {
            panic!("a line should stay a line");
        };
        assert!((x - 1000.0).abs() < 1e-9, "{x}");
        assert!((y - 1000.0).abs() < 1e-9, "{y}");
    }

    #[test]
    fn each_path_element_becomes_its_own_outline() {
        let outlines = outlines_of("<path d=\"M0 0\" /><path d=\"M1 1\" />").unwrap();
        assert_eq!(outlines.len(), 2);
    }

    #[test]
    fn a_generated_glyph_names_its_source_icon() {
        let entry = Entry {
            glyph: "Trash".to_owned(),
            icon: "trash".to_owned(),
        };
        let mut out = String::new();
        super::write_glyph(&mut out, &entry, &[vec![Step::Move(0.0, 0.0)]]);
        assert!(out.contains("`trash` from Tabler Icons"), "{out}");
        assert!(out.contains("static TRASH: &[&[Cmd]]"), "{out}");
        assert!(out.contains("Cmd::Move(0, 0)"), "{out}");
    }
}
