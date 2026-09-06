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
    icon: String,
}

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
        let _ = writeln!(
            out,
            "        Glyph::{} => {},",
            entry.glyph,
            shout(&entry.glyph)
        );
    }
    out.push_str(
        "        Glyph::ChessWhiteKing\n\
         | Glyph::ChessWhiteQueen\n\
         | Glyph::ChessWhiteRook\n\
         | Glyph::ChessWhiteBishop\n\
         | Glyph::ChessWhiteKnight\n\
         | Glyph::ChessWhitePawn\n\
         | Glyph::ChessBlackKing\n\
         | Glyph::ChessBlackQueen\n\
         | Glyph::ChessBlackRook\n\
         | Glyph::ChessBlackBishop\n\
         | Glyph::ChessBlackKnight\n\
         | Glyph::ChessBlackPawn\n\
         | Glyph::BlackDisc\n\
         | Glyph::WhiteDisc\n\
         | Glyph::BlackDraughtsKing\n\
         | Glyph::WhiteDraughtsKing\n\
         | Glyph::BlackDraughtsMan\n\
         | Glyph::WhiteDraughtsMan\n\
         | Glyph::MorrisPoint\n\
         | Glyph::MorrisLegalPoint => &[],\n",
    );
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
    use super::{outlines_of, read_manifest, shout, Entry, Step};

    #[test]
    fn a_manifest_line_is_a_glyph_and_an_icon() {
        let entries = read_manifest("# a note\n\nTrash trash\nClose x\n").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].glyph, "Trash");
        assert_eq!(entries[1].icon, "x");
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
