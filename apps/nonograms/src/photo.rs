//! Photo-to-puzzle conversion shared by the app's local transfer route.

use crate::corpus::Puzzle;
use kobo_image::{decode, Picture};

pub const MIN_SIDE: usize = 5;
pub const MAX_SIDE: usize = 25;
const REVEAL_WIDTH: u32 = 536;
const REVEAL_HEIGHT: u32 = 724;

/// The manifest a push writes beside the photos it names: one line per
/// puzzle, tab separated as file, name and grid side. A line format for the
/// reason the rest of the platform uses one: it is readable over the shell,
/// and a line that cannot be understood costs one puzzle rather than the
/// import.
pub const MANIFEST_FILE: &str = "imported.txt";

/// How many puzzles one import may carry. The manifest is a transfer
/// format, not an archive: past a screenful or two the answer is a second
/// push, not a longer file.
pub const MAX_IMPORTED: usize = 12;

/// The longest puzzle name a manifest may carry, in characters.
const MAX_NAME: usize = 48;

/// One puzzle the manifest names: the photo to read, what to call the
/// puzzle, and the grid to cut it into.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub file: String,
    pub name: String,
    pub side: usize,
}

pub fn parse_manifest(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "The manifest is not text.".to_owned())?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Err("The manifest names no puzzles.".to_owned());
    }
    if lines.len() > MAX_IMPORTED {
        return Err(format!(
            "The manifest names more than {MAX_IMPORTED} puzzles."
        ));
    }
    lines.into_iter().map(entry).collect()
}

fn entry(line: &str) -> Result<Entry, String> {
    let mut fields = line.split('\t');
    let (Some(file), Some(name), Some(side), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err("A manifest line is not file, name and size.".to_owned());
    };
    // The file must be a plain name inside the transfer directory: anything
    // with a path in it is asking to read somewhere else.
    let is_png = std::path::Path::new(file)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !is_png || file == ".png" || file.contains(['/', '\\']) {
        return Err(format!("{file} is not a photo file name."));
    }
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err("A manifest name is empty or too long.".to_owned());
    }
    let side: usize = side
        .parse()
        .map_err(|_| format!("{name} has no grid size."))?;
    if !(MIN_SIDE..=MAX_SIDE).contains(&side) {
        return Err(format!(
            "{name} asks for a grid outside {MIN_SIDE}\u{d7}{MIN_SIDE} to {MAX_SIDE}\u{d7}{MAX_SIDE}."
        ));
    }
    Ok(Entry {
        file: file.to_owned(),
        name: name.to_owned(),
        side,
    })
}

#[derive(Debug)]
pub struct PhotoPuzzle {
    pub puzzle: Puzzle,
    pub reveal: Picture,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhotoError {
    InvalidSize,
    Image(String),
    Unfair,
}

impl std::fmt::Display for PhotoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize => write!(
                formatter,
                "Choose a grid size from {MIN_SIDE} to {MAX_SIDE}."
            ),
            Self::Image(error) => write!(formatter, "The photo could not be read: {error}"),
            Self::Unfair => formatter.write_str("This one does not make a fair puzzle."),
        }
    }
}

/// Creates an original, thresholded puzzle only when line solving proves it fair.
pub fn from_photo(
    id: impl Into<String>,
    title: impl Into<String>,
    source: &[u8],
    side: usize,
) -> Result<PhotoPuzzle, PhotoError> {
    if !(MIN_SIDE..=MAX_SIDE).contains(&side) {
        return Err(PhotoError::InvalidSize);
    }
    let source = decode(source).map_err(|error| PhotoError::Image(error.to_string()))?;
    let id = id.into();
    let title = title.into();
    let samples = sample(&source, side);
    let mean = samples.iter().map(|value| u32::from(*value)).sum::<u32>()
        / u32::try_from(samples.len()).unwrap_or(1);
    // A photo with a forgiving threshold gets one of these passes. We refuse
    // rather than invent a solution that needs guessing.
    for offset in [-48_i32, -32, -16, 0, 16, 32, 48] {
        let threshold = u8::try_from((i32::try_from(mean).unwrap_or(128) + offset).clamp(16, 239))
            .unwrap_or(128);
        let puzzle = Puzzle {
            id: id.clone(),
            title: title.clone(),
            side,
            answer: samples.iter().map(|value| *value < threshold).collect(),
        };
        if puzzle.is_line_solvable() {
            return Ok(PhotoPuzzle {
                puzzle,
                reveal: reveal(&source)?,
            });
        }
    }
    Err(PhotoError::Unfair)
}

fn sample(source: &Picture, side: usize) -> Vec<u8> {
    let width = source.width() as usize;
    let height = source.height() as usize;
    (0..side)
        .flat_map(|y| {
            (0..side).map(move |x| {
                let left = x * width / side;
                let right = ((x + 1) * width / side).max(left + 1);
                let top = y * height / side;
                let bottom = ((y + 1) * height / side).max(top + 1);
                let mut total = 0_u64;
                let mut count = 0_u64;
                for pixel_y in top..bottom {
                    for pixel_x in left..right {
                        total += u64::from(source.grey()[pixel_y * width + pixel_x]);
                        count += 1;
                    }
                }
                u8::try_from(total / count.max(1)).unwrap_or(u8::MAX)
            })
        })
        .collect()
}

fn reveal(source: &Picture) -> Result<Picture, PhotoError> {
    let fitted = source
        .fit(REVEAL_WIDTH, REVEAL_HEIGHT)
        .map_err(|error| PhotoError::Image(error.to_string()))?;
    let grey = fitted
        .grey()
        .iter()
        .map(|pixel| pixel / 17 * 17)
        .collect::<Vec<_>>();
    Picture::from_grey(fitted.width(), fitted.height(), grey)
        .map_err(|error| PhotoError::Image(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        from_photo, parse_manifest, Entry, PhotoError, MAX_IMPORTED, MAX_NAME as MAX_NAME_LIMIT,
    };
    use kobo_image::Picture;

    fn encoded_gradient() -> Vec<u8> {
        let grey = (0..100)
            .map(|index| if index / 10 < 5 { 24 } else { 232 })
            .collect();
        let picture = Picture::from_grey(10, 10, grey).expect("picture");
        kobo_image::encode_png_grey(picture.width(), picture.height(), picture.grey()).expect("png")
    }

    fn encoded_ambiguous_diagonal() -> Vec<u8> {
        let grey = (0..25)
            .map(|index| if index / 5 == index % 5 { 0 } else { 255 })
            .collect();
        let picture = Picture::from_grey(5, 5, grey).expect("picture");
        kobo_image::encode_png_grey(picture.width(), picture.height(), picture.grey()).expect("png")
    }

    #[test]
    fn a_simple_photo_generates_a_fair_puzzle_and_a_sixteen_grey_reveal() {
        let photo = from_photo("photo", "Photo puzzle", &encoded_gradient(), 5).expect("fair");
        assert!(photo.puzzle.is_line_solvable());
        assert!(photo.reveal.grey().iter().all(|pixel| pixel % 17 == 0));
    }

    #[test]
    fn a_manifest_names_each_puzzle_once() {
        let manifest = b"bird.png\tBird at dawn\t7\ncat.png\tCat\t25\n";
        let entries = parse_manifest(manifest).expect("manifest");
        assert_eq!(
            entries,
            vec![
                Entry {
                    file: "bird.png".into(),
                    name: "Bird at dawn".into(),
                    side: 7,
                },
                Entry {
                    file: "cat.png".into(),
                    name: "Cat".into(),
                    side: 25,
                },
            ]
        );
    }

    #[test]
    fn a_manifest_is_refused_rather_than_stretched() {
        // Over the bound.
        let long = (0..=MAX_IMPORTED).fold(String::new(), |mut text, n| {
            use std::fmt::Write as _;
            let _ = writeln!(text, "p{n}.png\tPuzzle {n}\t5");
            text
        });
        assert!(parse_manifest(long.as_bytes())
            .unwrap_err()
            .contains("more than"));
        // A grid outside the contract, and a size that is not a number.
        assert!(parse_manifest(b"p.png\tP\t4\n")
            .unwrap_err()
            .contains("outside"));
        assert!(parse_manifest(b"p.png\tP\t26\n")
            .unwrap_err()
            .contains("outside"));
        assert!(parse_manifest(b"p.png\tP\tnine\n")
            .unwrap_err()
            .contains("no grid size"));
        // A path is not a file name: the read stays inside the directory.
        assert!(parse_manifest(b"../secret.png\tP\t5\n")
            .unwrap_err()
            .contains("file name"));
        assert!(parse_manifest(b"a/b.png\tP\t5\n")
            .unwrap_err()
            .contains("file name"));
        assert!(parse_manifest(b"p.jpg\tP\t5\n")
            .unwrap_err()
            .contains("file name"));
        // A name that never ends, and a line missing a field.
        let name = "a".repeat(MAX_NAME_LIMIT + 1);
        assert!(parse_manifest(format!("p.png\t{name}\t5\n").as_bytes())
            .unwrap_err()
            .contains("too long"));
        assert!(parse_manifest(b"p.png\tOnly two\n")
            .unwrap_err()
            .contains("file, name and size"));
        // Nothing at all.
        assert!(parse_manifest(b"\n").unwrap_err().contains("no puzzles"));
    }

    #[test]
    fn size_gate_and_unfair_photo_are_refused() {
        assert_eq!(
            from_photo("photo", "Photo puzzle", &encoded_gradient(), 4).unwrap_err(),
            PhotoError::InvalidSize
        );
        assert_eq!(
            from_photo("diagonal", "Diagonal", &encoded_ambiguous_diagonal(), 5).unwrap_err(),
            PhotoError::Unfair
        );
    }
}
