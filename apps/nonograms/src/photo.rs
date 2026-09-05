//! Photo-to-puzzle conversion shared by the app's local transfer route.

use crate::corpus::Puzzle;
use kobo_image::{decode, Picture};

pub const MIN_SIDE: usize = 5;
pub const MAX_SIDE: usize = 25;
const REVEAL_WIDTH: u32 = 536;
const REVEAL_HEIGHT: u32 = 724;

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
    use super::{from_photo, PhotoError};
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
