//! Bounded CBZ inspection using Cobalt's EPUB ZIP reader.

use kobo_doc::zip::Archive;
use kobo_image::{decode, ImageError};
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comic {
    pub pages: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComicError {
    Archive(String),
    Empty,
    Image(String),
}

impl std::fmt::Display for ComicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Archive(reason) => write!(f, "CBZ could not be opened: {reason}"),
            Self::Empty => f.write_str("CBZ contains no PNG or JPEG pages."),
            Self::Image(reason) => write!(f, "A comic page could not be decoded: {reason}"),
        }
    }
}

pub fn inspect(bytes: &[u8]) -> Result<Comic, ComicError> {
    let archive = Archive::open(bytes).map_err(|error| ComicError::Archive(error.to_string()))?;
    let mut pages = archive
        .names()
        .filter(|name| {
            Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg"
                    )
                })
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    pages.sort_unstable();
    if pages.is_empty() {
        Err(ComicError::Empty)
    } else {
        Ok(Comic { pages })
    }
}

pub fn page(bytes: &[u8], comic: &Comic, index: usize) -> Result<kobo_image::Picture, ComicError> {
    let archive = Archive::open(bytes).map_err(|error| ComicError::Archive(error.to_string()))?;
    let name = comic.pages.get(index).ok_or(ComicError::Empty)?;
    let encoded = archive
        .read(name)
        .map_err(|error| ComicError::Archive(error.to_string()))?;
    decode(&encoded).map_err(|error: ImageError| ComicError::Image(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::inspect;

    #[test]
    fn page_names_are_sorted_like_a_volume() {
        let cbz = kobo_doc::zip::stored(&[
            ("10.jpg".to_owned(), vec![1]),
            ("02.png".to_owned(), vec![2]),
            ("notes.txt".to_owned(), vec![3]),
        ])
        .expect("zip");
        assert_eq!(inspect(&cbz).expect("comic").pages, ["02.png", "10.jpg"]);
    }
}
