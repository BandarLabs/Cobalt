//! Covers read out of the books themselves, and what is kept of them.
//!
//! Every EPUB on the shelf used to wear the same painted placeholder: a band
//! of stripes and a black disc, identical for a novel and a manual. The books
//! carry their own covers, and the stock reader shows them, so a shelf that
//! does not is the first thing an owner notices. Reading a cover means reading
//! the book, which on this hardware is the slow part, so what is learned is
//! kept: a small picture per book in the store, and one record of what each
//! book is called and who wrote it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use kobo_image::Picture;

/// A cover drawn to a tile's size, in the panel's greys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub grey: Vec<u8>,
}

impl Thumb {
    /// The same picture at another size, cropped to fill it.
    fn scaled(&self, width: u32, height: u32) -> Option<Self> {
        let picture = Picture::from_grey(self.width, self.height, self.grey.clone()).ok()?;
        let fitted = picture.cover(width, height).ok()?;
        Some(Self {
            width: fitted.width(),
            height: fitted.height(),
            grey: fitted.into_grey(),
        })
    }
}

/// The smallest picture taken for a cover.
///
/// A publisher's mark or a spacer image is sometimes all a book names as its
/// cover. Blown up to a tile it is a grey smear, which is worse than the
/// lettered cover the shelf draws instead.
const MIN_COVER_PX: u32 = 96;

/// Draws a book's own cover picture to fill a tile.
#[must_use]
pub fn thumbnail(cover: &[u8], width: u32, height: u32) -> Option<Thumb> {
    let picture = kobo_image::decode(cover).ok()?;
    if picture.width() < MIN_COVER_PX || picture.height() < MIN_COVER_PX {
        return None;
    }
    let mut fitted = picture.cover(width, height).ok()?;
    fitted.dither(kobo_image::PANEL_GREYS);
    Some(Thumb {
        width: fitted.width(),
        height: fitted.height(),
        grey: fitted.into_grey(),
    })
}

/// A kept cover, as the store holds it.
#[must_use]
pub fn to_png(thumb: &Thumb) -> Option<Vec<u8>> {
    kobo_image::encode_png_grey(thumb.width, thumb.height, &thumb.grey).ok()
}

/// A kept cover read back.
#[must_use]
pub fn from_png(bytes: &[u8]) -> Option<Thumb> {
    let picture = kobo_image::decode(bytes).ok()?;
    Some(Thumb {
        width: picture.width(),
        height: picture.height(),
        grey: picture.into_grey(),
    })
}

/// The store key a book's cover is kept under.
///
/// Named by the book, its size and the name it was listed under, so a file
/// replaced by another book gets its own cover rather than the old one, and
/// digested so that no path from the card is written into the store.
#[must_use]
pub fn key(id: &str, bytes: u32, listed: &str) -> String {
    let digest = kobo_bookview::positions::identity(&format!("cover:{id}:{bytes}:{listed}"));
    format!("cover-{}", &digest[..32])
}

/// A few covers stacked into one picture, the way a pile of books looks.
///
/// The stock reader's home shows its library as covers fanned on top of each
/// other, and a shelf card that is one more glyph says nothing about what is
/// on that shelf. The first cover is in front and the rest step back to the
/// right, a little smaller each time, so the pile reads as depth rather than
/// as three pictures that happen to overlap. Each one is drawn with a white
/// gap and a hairline around it, because two covers that meet with no edge
/// between them on a greyscale panel read as one picture.
///
/// A book behind the first whose cover is not known is drawn as a plain
/// board. A lettered cover there showed the middle of its title, cut off by
/// the book in front, as a stray fragment of a word.
#[must_use]
pub fn stack(covers: &[Option<&Thumb>], width: u32, height: u32) -> Vec<u8> {
    let (canvas_width, canvas_height) = (width as usize, height as usize);
    let mut canvas = vec![0xFF_u8; canvas_width * canvas_height];
    let count = u32::try_from(covers.len().min(3)).unwrap_or(3);
    if count == 0 || width == 0 || height == 0 {
        return canvas;
    }
    // Two thirds as wide as tall is the shape of nearly every printed book.
    let mut front_height = height * 86 / 100;
    let mut front_width = front_height * 2 / 3;
    if front_width > width * 55 / 100 {
        front_width = width * 55 / 100;
        front_height = front_width * 3 / 2;
    }
    let step = if count > 1 {
        ((width * 92 / 100).saturating_sub(front_width) / (count - 1)).min(front_width * 55 / 100)
    } else {
        0
    };
    let spread = front_width + step * (count - 1);
    let left = (width.saturating_sub(spread)) / 2;
    let bottom = (height + front_height) / 2;
    // Drawn from the back, so the first cover ends up on top of the pile.
    for (place, cover) in (0..count).zip(covers).rev() {
        let shrink = 100 - 9 * place;
        let cover_width = front_width * shrink / 100;
        let cover_height = front_height * shrink / 100;
        let scaled = match cover {
            Some(cover) => cover.scaled(cover_width, cover_height),
            None => Some(Thumb {
                width: cover_width,
                height: cover_height,
                grey: vec![BOARD; (cover_width * cover_height) as usize],
            }),
        };
        let Some(scaled) = scaled else {
            continue;
        };
        let x = left + step * place;
        let y = bottom.saturating_sub(cover_height);
        paint(
            &mut canvas,
            canvas_width,
            canvas_height,
            &scaled,
            x as usize,
            y as usize,
        );
    }
    canvas
}

/// The tone of a book whose cover is not known yet.
const BOARD: u8 = 0xD8;

/// Copies one cover onto the canvas with its gap and its edge.
fn paint(canvas: &mut [u8], width: usize, height: usize, cover: &Thumb, x: usize, y: usize) {
    const GAP: usize = 3;
    const EDGE: u8 = 0x30;
    let (cover_width, cover_height) = (cover.width as usize, cover.height as usize);
    let left = x.saturating_sub(GAP);
    let top = y.saturating_sub(GAP);
    let right = (x + cover_width + GAP).min(width);
    let bottom = (y + cover_height + GAP).min(height);
    for row in top..bottom {
        for column in left..right {
            canvas[row * width + column] = 0xFF;
        }
    }
    for row in 0..cover_height {
        let canvas_row = y + row;
        if canvas_row >= height {
            break;
        }
        for column in 0..cover_width {
            let canvas_column = x + column;
            if canvas_column >= width {
                break;
            }
            let edge =
                row == 0 || column == 0 || row + 1 == cover_height || column + 1 == cover_width;
            canvas[canvas_row * width + canvas_column] = if edge {
                EDGE
            } else {
                cover.grey[row * cover_width + column]
            };
        }
    }
}

/// Whether a book's cover has been read, and where it went.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cover {
    /// The book names no cover, or names one that will not decode.
    Missing,
    /// Read, and kept in the store under this key.
    Kept(String),
    /// Read, and not kept: the store had no room for it.
    Unkept,
}

/// What reading a book once told the shelf about it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Described {
    /// The size the book was when it was read, and the name the runtime
    /// listed it under then, so a replaced file is read again rather than
    /// shown under the old book's name.
    pub bytes: u32,
    pub listed: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub cover: Cover,
}

impl Described {
    /// Whether this record was made from the version of the book now listed.
    #[must_use]
    pub fn is_of(&self, bytes: u32, listed: &str) -> bool {
        self.bytes == bytes && self.listed == listed
    }
}

/// Store key for what reading each book told the shelf, and the first of
/// its parts.
pub const DESCRIBED: &str = "described-v1";

/// How many store values the record of described books may take.
///
/// It was one, which was room for the 400 books a listing used to stop at.
/// A listing now runs to 2,000, and a record of that many is past the 256
/// KiB a value holds: it was encoded, found too large, and silently not
/// kept, so every cover was read out of its book again on every start.
///
/// A line is usually under 250 bytes and at most about 1,400, an identifier
/// of 256, a listed name of 96, and a title and an author of
/// [`MOST_NAME`] characters each. Twelve values hold 2,000 of the longest
/// lines and several times that many ordinary ones, and leave the store its
/// room for 200 covers. A record that still does not fit is not kept, which
/// costs covers read again and never a shelf that fails.
pub const DESCRIBED_PARTS: usize = 12;

/// The longest title or author kept, in characters. A tile shows a fraction
/// of this, and a publisher's paragraph in the title field is otherwise a
/// kilobyte kept for every start.
const MOST_NAME: usize = 120;

/// The store key of part `index` of the record of described books.
#[must_use]
pub fn described_key(index: usize) -> String {
    if index == 0 {
        DESCRIBED.to_owned()
    } else {
        format!("{DESCRIBED}.{index}")
    }
}

/// Which part of the record of described books `key` is, if it is one.
#[must_use]
pub fn described_part(key: &str) -> Option<usize> {
    if key == DESCRIBED {
        return Some(0);
    }
    let index: usize = key
        .strip_prefix(DESCRIBED)?
        .strip_prefix('.')?
        .parse()
        .ok()?;
    (1..DESCRIBED_PARTS).contains(&index).then_some(index)
}

/// The record of described books as store values of at most `most` bytes
/// each: a line a book, tab-separated, never splitting a line. `None` when
/// it needs more than [`DESCRIBED_PARTS`] of them.
#[must_use]
pub fn encode_described(books: &BTreeMap<String, Described>, most: usize) -> Option<Vec<Vec<u8>>> {
    let clean = |text: Option<&str>| -> String {
        text.unwrap_or_default()
            .chars()
            .take(MOST_NAME)
            .map(|c| {
                if matches!(c, '\t' | '\n' | '\r') {
                    ' '
                } else {
                    c
                }
            })
            .collect()
    };
    let mut parts = vec![Vec::new()];
    let mut line = String::new();
    for (id, book) in books {
        let cover = match &book.cover {
            Cover::Missing => "!",
            Cover::Unkept => "?",
            Cover::Kept(key) => key.as_str(),
        };
        line.clear();
        let _ = writeln!(
            line,
            "{id}\t{}\t{}\t{}\t{}\t{cover}",
            book.bytes,
            clean(Some(&book.listed)),
            clean(book.title.as_deref()),
            clean(book.author.as_deref())
        );
        let full = parts
            .last()
            .is_some_and(|part| part.len() + line.len() > most);
        if full {
            parts.push(Vec::new());
        }
        parts.last_mut()?.extend_from_slice(line.as_bytes());
    }
    (parts.len() <= DESCRIBED_PARTS).then_some(parts)
}

/// The record read back. A line that does not parse is dropped, which costs
/// one book being read again, not the whole shelf.
#[must_use]
pub fn decode_described(bytes: &[u8]) -> BTreeMap<String, Described> {
    let text = String::from_utf8_lossy(bytes);
    let named = |text: &str| (!text.is_empty()).then(|| text.to_owned());
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?.to_owned();
            let bytes = fields.next()?.parse().ok()?;
            let listed = fields.next()?.to_owned();
            let title = named(fields.next()?);
            let author = named(fields.next()?);
            let cover = match fields.next()? {
                "!" => Cover::Missing,
                "?" => Cover::Unkept,
                key if key.starts_with("cover-") => Cover::Kept(key.to_owned()),
                _ => return None,
            };
            Some((
                id,
                Described {
                    bytes,
                    listed,
                    title,
                    author,
                    cover,
                },
            ))
        })
        .collect()
}

/// A book opened lately, and the page it was closed on.
///
/// The page and the count are the reader's own at the size the book was set
/// in, which is what the owner saw at the foot of the page when they left.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recent {
    pub id: String,
    pub page: u32,
    pub pages: u32,
}

impl Recent {
    /// How far in, as a whole percentage. A book is not finished until its
    /// last page, so the rounding is down.
    #[must_use]
    pub fn percent(&self) -> u8 {
        let pages = u64::from(self.pages.max(1));
        let page = u64::from(self.page.min(self.pages));
        u8::try_from(page * 100 / pages).unwrap_or(100)
    }
}

/// The books opened most recently, newest first, as bytes.
#[must_use]
pub fn encode_recent(recent: &[Recent]) -> Vec<u8> {
    let mut out = String::new();
    for book in recent {
        let _ = writeln!(out, "{}\t{}\t{}", book.id, book.page, book.pages);
    }
    out.into_bytes()
}

#[must_use]
pub fn decode_recent(bytes: &[u8]) -> Vec<Recent> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(Recent {
                id: fields.next()?.to_owned(),
                page: fields.next()?.parse().ok()?,
                pages: fields.next()?.parse().ok()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        decode_described, decode_recent, encode_described, encode_recent, key, stack, Cover,
        Described, Recent, Thumb,
    };
    use std::collections::BTreeMap;

    #[test]
    fn what_was_learned_about_a_book_survives_the_store() {
        let mut books = BTreeMap::new();
        books.insert(
            "1/gutenbird/book-1a2b3c4d".to_owned(),
            Described {
                bytes: 535_571,
                listed: "book-1a2b3c4d".to_owned(),
                title: Some("The Time\tMachine".to_owned()),
                author: Some("H. G. Wells".to_owned()),
                cover: Cover::Kept(key("1/gutenbird/book-1a2b3c4d", 535_571, "book-1a2b3c4d")),
            },
        );
        books.insert(
            "0/notes.epub".to_owned(),
            Described {
                bytes: 80,
                listed: "notes".to_owned(),
                title: None,
                author: None,
                cover: Cover::Missing,
            },
        );
        let parts = encode_described(&books, 1 << 20).expect("fits");
        assert_eq!(parts.len(), 1);
        let read = decode_described(&parts[0]);
        assert_eq!(
            read["1/gutenbird/book-1a2b3c4d"].title.as_deref(),
            Some("The Time Machine"),
            "a tab in a title would have split the line"
        );
        assert_eq!(read["0/notes.epub"], books["0/notes.epub"]);
        assert_eq!(
            read["1/gutenbird/book-1a2b3c4d"].cover,
            books["1/gutenbird/book-1a2b3c4d"].cover
        );
    }

    #[test]
    fn the_recent_books_come_back_in_the_order_they_were_opened() {
        let recent = vec![
            Recent {
                id: "0/b.epub".to_owned(),
                page: 12,
                pages: 340,
            },
            Recent {
                id: "0/a.epub".to_owned(),
                page: 566,
                pages: 566,
            },
        ];
        assert_eq!(decode_recent(&encode_recent(&recent)), recent);
        assert_eq!(recent[0].percent(), 3);
        assert_eq!(recent[1].percent(), 100);
    }

    #[test]
    fn a_cover_key_fits_the_store_and_changes_with_the_file() {
        let first = key("0/a.epub", 10, "a");
        // The store refuses a key longer than 64 bytes.
        assert!(first.len() <= 64);
        assert!(kobo_sdk::is_valid_key(&first));
        assert_ne!(first, key("0/a.epub", 11, "a"));
        assert_ne!(first, key("0/a.epub", 10, "Another book"));
    }

    #[test]
    fn a_pile_of_covers_shows_each_one_and_keeps_the_first_on_top() {
        let dark = Thumb {
            width: 60,
            height: 90,
            grey: vec![0x00; 60 * 90],
        };
        let light = Thumb {
            width: 60,
            height: 90,
            grey: vec![0xA0; 60 * 90],
        };
        let canvas = stack(&[Some(&dark), Some(&light), Some(&light)], 240, 160);
        assert_eq!(canvas.len(), 240 * 160);
        let row = &canvas[100 * 240..101 * 240];
        let first_dark = row.iter().position(|pixel| *pixel == 0x00);
        let first_light = row.iter().position(|pixel| *pixel == 0xA0);
        assert!(
            first_dark.is_some() && first_light.is_some(),
            "a cover went missing"
        );
        assert!(
            first_dark < first_light,
            "the first cover is the one in front, at the left"
        );
    }

    #[test]
    fn the_record_of_a_full_library_is_kept_in_parts_and_read_back_whole() {
        use super::{described_key, described_part, DESCRIBED, DESCRIBED_PARTS};
        let books: BTreeMap<String, Described> = (0..2_000)
            .map(|n| {
                (
                    format!("0/{}/{n:04}.epub", "a".repeat(240)),
                    Described {
                        bytes: 4_000_000,
                        listed: "l".repeat(96),
                        title: Some("\u{00e9}".repeat(400)),
                        author: Some("\u{672c}".repeat(400)),
                        cover: Cover::Kept(key("0/book.epub", 1, "book")),
                    },
                )
            })
            .collect();
        let parts = encode_described(&books, kobo_sdk::MAX_STORE_VALUE)
            .expect("the longest lines of the largest listing fit");
        assert!(
            parts.len() > 1 && parts.len() <= DESCRIBED_PARTS,
            "{}",
            parts.len()
        );
        assert!(parts
            .iter()
            .all(|part| part.len() <= kobo_sdk::MAX_STORE_VALUE));
        let back: BTreeMap<String, Described> = parts
            .iter()
            .flat_map(|part| decode_described(part))
            .collect();
        assert_eq!(back.len(), books.len());
        let first = back.values().next().expect("a book");
        assert_eq!(
            first.title.as_ref().map(|title| title.chars().count()),
            Some(120)
        );
        assert_eq!(
            first.author.as_ref().map(|author| author.chars().count()),
            Some(120)
        );
        assert!(first.is_of(4_000_000, &"l".repeat(96)));
        assert_eq!(
            encode_described(&books, 4096),
            None,
            "more parts than there are"
        );

        assert_eq!(described_key(0), DESCRIBED);
        assert_eq!(described_part(&described_key(0)), Some(0));
        assert_eq!(
            described_part(&described_key(DESCRIBED_PARTS - 1)),
            Some(DESCRIBED_PARTS - 1)
        );
        assert_eq!(
            described_part(&format!("{DESCRIBED}.{DESCRIBED_PARTS}")),
            None
        );
        assert_eq!(described_part(&format!("{DESCRIBED}.0")), None);
        assert_eq!(described_part("described-v10"), None);
        assert_eq!(described_part("cover-0123"), None);
    }
}
