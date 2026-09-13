//! Durable metadata for verified shelf files; records are published after file acknowledgement.
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};

pub const KEY: &str = "books-v1";
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BOOKS: usize = 64;
pub const RECORD_LIMIT: usize = 192 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub title: String,
    pub authors: String,
    pub url: String,
    pub file: String,
    pub digest: String,
    pub size: usize,
    pub memory: String,
}
impl Book {
    pub fn from_bytes(
        title: &str,
        authors: &str,
        url: &str,
        bytes: &[u8],
        epub: bool,
    ) -> Result<Self, Error> {
        let digest = kobo_net::sha256::hex_digest(bytes);
        let book = Self {
            title: title.into(),
            authors: authors.into(),
            url: url.into(),
            file: format!("{}.{}", &digest[..56], if epub { "epub" } else { "txt" }),
            digest,
            size: bytes.len(),
            memory: String::new(),
        };
        book.validate()?;
        Ok(book)
    }
    fn validate(&self) -> Result<(), Error> {
        let suffix = self
            .file
            .strip_prefix(&format!("{}.", self.digest.get(..56).unwrap_or_default()));
        if !kobo_sdk::is_valid_key(&self.file)
            || self.title.trim().is_empty()
            || self.title.len() > 1024
            || self.authors.len() > 1024
            || self.url.len() > 2048
            || super::catalog::endpoint(&self.url).is_none()
            || self.size == 0
            || self.size > MAX_BYTES
            || self.memory.len() > 16 * 1024
            || self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(suffix, Some("epub" | "txt"))
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
    pub fn verifies(&self, bytes: &[u8]) -> bool {
        bytes.len() == self.size && kobo_net::sha256::hex_digest(bytes) == self.digest
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub books: Vec<Book>,
}
impl Library {
    pub fn restore(bytes: Option<&[u8]>) -> Result<Self, Error> {
        let Some(record) = Schema::new("calibre.books", 1, RECORD_LIMIT)?
            .restore(bytes, |_, _| Err(Error::MigrationUnavailable))?
        else {
            return Ok(Self::default());
        };
        let values = record
            .payload
            .get("books")
            .and_then(Value::as_array)
            .ok_or(Error::Corrupt)?;
        if values.len() > MAX_BOOKS {
            return Err(Error::Corrupt);
        }
        let mut books = Vec::new();
        for value in values {
            let text = |key| value.get(key).and_then(Value::as_str).ok_or(Error::Corrupt);
            let book = Book {
                title: text("title")?.into(),
                authors: text("authors")?.into(),
                url: text("url")?.into(),
                file: text("file")?.into(),
                digest: text("digest")?.into(),
                size: text("size")?.parse().map_err(|_| Error::Corrupt)?,
                memory: text("memory")?.into(),
            };
            book.validate()?;
            if books.iter().any(|other: &Book| other.file == book.file) {
                return Err(Error::Corrupt);
            }
            books.push(book);
        }
        Ok(Self { books })
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.books.len() > MAX_BOOKS {
            return Err(Error::Corrupt);
        }
        let mut values = Vec::new();
        for book in &self.books {
            book.validate()?;
            values.push(
                ObjectBuilder::new()
                    .set("title", book.title.as_str())
                    .set("authors", book.authors.as_str())
                    .set("url", book.url.as_str())
                    .set("file", book.file.as_str())
                    .set("digest", book.digest.as_str())
                    .set("size", book.size.to_string())
                    .set("memory", book.memory.as_str())
                    .build(),
            );
        }
        Schema::new("calibre.books", 1, RECORD_LIMIT)?.encode(
            &ObjectBuilder::new()
                .set("books", Value::Array(values))
                .build(),
        )
    }
    pub fn insert(&mut self, mut book: Book) -> Result<(), Error> {
        book.validate()?;
        let mut candidate = self.clone();
        if let Some(old) = candidate
            .books
            .iter_mut()
            .find(|old| old.file == book.file || old.url == book.url)
        {
            if book.file == old.file {
                book.memory.clone_from(&old.memory);
            }
            *old = book;
        } else {
            if candidate.books.len() == MAX_BOOKS {
                return Err(Error::TooLarge);
            }
            candidate.books.push(book);
        }
        candidate.encode()?;
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_content_reuses_file_and_position_but_changed_bytes_do_not() {
        let mut library = Library::default();
        let mut book = Book::from_bytes(
            "River",
            "Sample",
            "https://library/book",
            b"A short book.",
            false,
        )
        .unwrap();
        assert!(kobo_sdk::is_valid_key(&book.file));
        book.memory = "at 2\n".into();
        library.insert(book.clone()).unwrap();
        let copy = Book::from_bytes(
            "Another copy",
            "Sample",
            "https://library/copy",
            b"A short book.",
            false,
        )
        .unwrap();
        library.insert(copy).unwrap();
        assert_eq!(library.books.len(), 1);
        assert_eq!(library.books[0].memory, book.memory);
        assert!(book.verifies(b"A short book."));
        assert!(!book.verifies(b"Another book."));
        let encoded = library.encode().unwrap();
        assert_eq!(Library::restore(Some(&encoded)).unwrap(), library);
        let future = String::from_utf8(encoded)
            .unwrap()
            .replace("\"version\":1", "\"version\":99");
        assert!(Library::restore(Some(future.as_bytes())).is_err());
    }
    #[test]
    fn invalid_file_or_oversized_memory_cannot_replace_library() {
        let mut library = Library::default();
        let mut book =
            Book::from_bytes("River", "", "https://library/book", b"Text", false).unwrap();
        book.file = "../other.txt".into();
        assert!(library.insert(book).is_err());
        assert!(library.books.is_empty());
    }
}
