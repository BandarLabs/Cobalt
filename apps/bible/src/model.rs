#![forbid(unsafe_code)]

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Testament {
    Old,
    New,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Book {
    pub id: &'static str,
    pub name: &'static str,
    pub testament: Testament,
    pub chapters: u32,
}

pub const BOOKS: [Book; 66] = [
    // Old Testament (39 books)
    Book { id: "GEN", name: "Genesis", testament: Testament::Old, chapters: 50 },
    Book { id: "EXO", name: "Exodus", testament: Testament::Old, chapters: 40 },
    Book { id: "LEV", name: "Leviticus", testament: Testament::Old, chapters: 27 },
    Book { id: "NUM", name: "Numbers", testament: Testament::Old, chapters: 36 },
    Book { id: "DEU", name: "Deuteronomy", testament: Testament::Old, chapters: 34 },
    Book { id: "JOS", name: "Joshua", testament: Testament::Old, chapters: 24 },
    Book { id: "JDG", name: "Judges", testament: Testament::Old, chapters: 21 },
    Book { id: "RUT", name: "Ruth", testament: Testament::Old, chapters: 4 },
    Book { id: "1SA", name: "1 Samuel", testament: Testament::Old, chapters: 31 },
    Book { id: "2SA", name: "2 Samuel", testament: Testament::Old, chapters: 24 },
    Book { id: "1KI", name: "1 Kings", testament: Testament::Old, chapters: 22 },
    Book { id: "2KI", name: "2 Kings", testament: Testament::Old, chapters: 25 },
    Book { id: "1CH", name: "1 Chronicles", testament: Testament::Old, chapters: 29 },
    Book { id: "2CH", name: "2 Chronicles", testament: Testament::Old, chapters: 36 },
    Book { id: "EZR", name: "Ezra", testament: Testament::Old, chapters: 10 },
    Book { id: "NEH", name: "Nehemiah", testament: Testament::Old, chapters: 13 },
    Book { id: "EST", name: "Esther", testament: Testament::Old, chapters: 10 },
    Book { id: "JOB", name: "Job", testament: Testament::Old, chapters: 42 },
    Book { id: "PSA", name: "Psalms", testament: Testament::Old, chapters: 150 },
    Book { id: "PRO", name: "Proverbs", testament: Testament::Old, chapters: 31 },
    Book { id: "ECC", name: "Ecclesiastes", testament: Testament::Old, chapters: 12 },
    Book { id: "SNG", name: "Song of Solomon", testament: Testament::Old, chapters: 8 },
    Book { id: "ISA", name: "Isaiah", testament: Testament::Old, chapters: 66 },
    Book { id: "JER", name: "Jeremiah", testament: Testament::Old, chapters: 52 },
    Book { id: "LAM", name: "Lamentations", testament: Testament::Old, chapters: 5 },
    Book { id: "EZK", name: "Ezekiel", testament: Testament::Old, chapters: 48 },
    Book { id: "DAN", name: "Daniel", testament: Testament::Old, chapters: 12 },
    Book { id: "HOS", name: "Hosea", testament: Testament::Old, chapters: 14 },
    Book { id: "JOL", name: "Joel", testament: Testament::Old, chapters: 3 },
    Book { id: "AMO", name: "Amos", testament: Testament::Old, chapters: 9 },
    Book { id: "OBA", name: "Obadiah", testament: Testament::Old, chapters: 1 },
    Book { id: "JON", name: "Jonah", testament: Testament::Old, chapters: 4 },
    Book { id: "MIC", name: "Micah", testament: Testament::Old, chapters: 7 },
    Book { id: "NAM", name: "Nahum", testament: Testament::Old, chapters: 3 },
    Book { id: "HAB", name: "Habakkuk", testament: Testament::Old, chapters: 3 },
    Book { id: "ZEP", name: "Zephaniah", testament: Testament::Old, chapters: 3 },
    Book { id: "HAG", name: "Haggai", testament: Testament::Old, chapters: 2 },
    Book { id: "ZEC", name: "Zechariah", testament: Testament::Old, chapters: 14 },
    Book { id: "MAL", name: "Malachi", testament: Testament::Old, chapters: 4 },
    // New Testament (27 books)
    Book { id: "MAT", name: "Matthew", testament: Testament::New, chapters: 28 },
    Book { id: "MRK", name: "Mark", testament: Testament::New, chapters: 16 },
    Book { id: "LUK", name: "Luke", testament: Testament::New, chapters: 24 },
    Book { id: "JHN", name: "John", testament: Testament::New, chapters: 21 },
    Book { id: "ACT", name: "Acts", testament: Testament::New, chapters: 28 },
    Book { id: "ROM", name: "Romans", testament: Testament::New, chapters: 16 },
    Book { id: "1CO", name: "1 Corinthians", testament: Testament::New, chapters: 16 },
    Book { id: "2CO", name: "2 Corinthians", testament: Testament::New, chapters: 13 },
    Book { id: "GAL", name: "Galatians", testament: Testament::New, chapters: 6 },
    Book { id: "EPH", name: "Ephesians", testament: Testament::New, chapters: 6 },
    Book { id: "PHP", name: "Philippians", testament: Testament::New, chapters: 4 },
    Book { id: "COL", name: "Colossians", testament: Testament::New, chapters: 4 },
    Book { id: "1TH", name: "1 Thessalonians", testament: Testament::New, chapters: 5 },
    Book { id: "2TH", name: "2 Thessalonians", testament: Testament::New, chapters: 3 },
    Book { id: "1TI", name: "1 Timothy", testament: Testament::New, chapters: 6 },
    Book { id: "2TI", name: "2 Timothy", testament: Testament::New, chapters: 4 },
    Book { id: "TIT", name: "Titus", testament: Testament::New, chapters: 3 },
    Book { id: "PHM", name: "Philemon", testament: Testament::New, chapters: 1 },
    Book { id: "HEB", name: "Hebrews", testament: Testament::New, chapters: 13 },
    Book { id: "JAS", name: "James", testament: Testament::New, chapters: 5 },
    Book { id: "1PE", name: "1 Peter", testament: Testament::New, chapters: 5 },
    Book { id: "2PE", name: "2 Peter", testament: Testament::New, chapters: 3 },
    Book { id: "1JN", name: "1 John", testament: Testament::New, chapters: 5 },
    Book { id: "2JN", name: "2 John", testament: Testament::New, chapters: 1 },
    Book { id: "3JN", name: "3 John", testament: Testament::New, chapters: 1 },
    Book { id: "JUD", name: "Jude", testament: Testament::New, chapters: 1 },
    Book { id: "REV", name: "Revelation", testament: Testament::New, chapters: 22 },
];

pub const DEFAULT_BOOK_INDEX: usize = 40; // Mark (0-indexed)

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Translation {
    #[default]
    Bsb,
    Web,
    Kjv,
}

impl Translation {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Bsb => "BSB",
            Self::Web => "WEB",
            Self::Kjv => "KJV",
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Bsb => "Berean Standard Bible (BSB)",
            Self::Web => "World English Bible (WEB)",
            Self::Kjv => "King James Version (KJV)",
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "WEB" => Self::Web,
            "KJV" => Self::Kjv,
            _ => Self::Bsb,
        }
    }
}

impl fmt::Display for Translation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id())
    }
}

#[allow(dead_code)]
#[must_use]
pub fn find_book_by_id(id: &str) -> Option<usize> {
    BOOKS.iter().position(|b| b.id.eq_ignore_ascii_case(id))
}

#[must_use]
pub fn superscript_verse_number(n: u32) -> String {
    let s = n.to_string();
    s.chars()
        .map(|c| match c {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            _ => c,
        })
        .collect()
}
