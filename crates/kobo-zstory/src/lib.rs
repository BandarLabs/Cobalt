//! Shared, bounded structural inspection for Parser Z-machine stories.

use std::fmt;
use std::path::Path;

const HEADER_LEN: usize = 64;
pub const MAX_STORY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoryError {
    Empty,
    TooShort,
    TooLarge,
    Glulx,
    UnsupportedVersion(u8),
    Invalid(&'static str),
}

impl fmt::Display for StoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("the file is empty"),
            Self::TooShort => formatter.write_str("the file is too short to be a Z-machine story"),
            Self::TooLarge => write!(
                formatter,
                "the story exceeds the {} MiB Parser limit",
                MAX_STORY_BYTES / (1024 * 1024)
            ),
            Self::Glulx => {
                formatter.write_str("this is a Glulx story - Parser does not support it")
            }
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "Z-machine version {version} is not supported; Parser accepts v3, v5 and v8"
            ),
            Self::Invalid(reason) => write!(formatter, "invalid Z-machine story: {reason}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoryInfo {
    pub version: u8,
    pub release: u16,
    pub serial: [u8; 6],
    pub checksum: u16,
    pub title: String,
    pub id: String,
    pub bytes: usize,
}

impl StoryInfo {
    /// Inspect the structural header facts shared by the host and interpreter.
    ///
    /// # Errors
    /// Returns a named format or structural error when Parser cannot safely open the bytes.
    pub fn inspect(bytes: &[u8], file_name: &str) -> Result<Self, StoryError> {
        if bytes.is_empty() {
            return Err(StoryError::Empty);
        }
        if bytes.starts_with(b"Glul") {
            return Err(StoryError::Glulx);
        }
        if bytes.len() < HEADER_LEN {
            return Err(StoryError::TooShort);
        }
        if bytes.len() > MAX_STORY_BYTES {
            return Err(StoryError::TooLarge);
        }
        let version = bytes[0];
        if !matches!(version, 3 | 5 | 8) {
            return Err(StoryError::UnsupportedVersion(version));
        }
        let release = word(bytes, 2)?;
        let serial: [u8; 6] = bytes[0x12..0x18]
            .try_into()
            .map_err(|_| StoryError::Invalid("missing serial number"))?;
        let checksum = word(bytes, 0x1c)?;
        let scale = match version {
            3 => 2,
            5 => 4,
            8 => 8,
            _ => unreachable!(),
        };
        let declared = usize::from(word(bytes, 0x1a)?).saturating_mul(scale);
        if declared != 0 && declared > bytes.len() {
            return Err(StoryError::Invalid("declared length exceeds the file"));
        }
        let title = title(file_name, release);
        let id = format!(
            "{release}-{}-{checksum:04x}",
            String::from_utf8_lossy(&serial)
        );
        Ok(Self {
            version,
            release,
            serial,
            checksum,
            title,
            id,
            bytes: bytes.len(),
        })
    }

    #[must_use]
    pub fn format(&self) -> String {
        format!("Z-machine v{}", self.version)
    }

    #[must_use]
    pub fn compatibility(&self) -> &'static str {
        "Playable in Parser's text-only v3/v5/v8 interpreter"
    }

    #[must_use]
    pub fn shelf_name(&self, original: &str) -> String {
        let extension = format!(".z{}", self.version);
        let mut stem = self
            .title
            .to_ascii_lowercase()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        while stem.contains("--") {
            stem = stem.replace("--", "-");
        }
        stem = stem.trim_matches('-').to_owned();
        if stem.is_empty() {
            "story".clone_into(&mut stem);
        }
        stem.truncate(38);
        let identity = self
            .id
            .to_ascii_lowercase()
            .replace(|character: char| !character.is_ascii_alphanumeric(), "-");
        let _ = original;
        format!(
            "story-{stem}-{}{}",
            &identity[..identity.len().min(18)],
            extension
        )
    }
}

fn title(file_name: &str, release: u16) -> String {
    let name = Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name);
    let stem = Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name);
    let title = stem
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        format!("Story {release}")
    } else {
        title
    }
}

fn word(bytes: &[u8], address: usize) -> Result<u16, StoryError> {
    let pair: [u8; 2] = bytes
        .get(address..address + 2)
        .ok_or(StoryError::Invalid("truncated header word"))?
        .try_into()
        .map_err(|_| StoryError::Invalid("truncated header word"))?;
    Ok(u16::from_be_bytes(pair))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn story(version: u8) -> Vec<u8> {
        let mut b = vec![0; 768];
        b[0] = version;
        b[2..4].copy_from_slice(&7u16.to_be_bytes());
        b[0x12..0x18].copy_from_slice(b"260917");
        let length = u16::try_from(b.len()).unwrap();
        b[0x1a..0x1c].copy_from_slice(
            &(length
                / match version {
                    3 => 2,
                    5 => 4,
                    _ => 8,
                })
            .to_be_bytes(),
        );
        b[0x1c..0x1e].copy_from_slice(&0x1234u16.to_be_bytes());
        b
    }
    #[test]
    fn supported_headers_report_owner_facing_facts() {
        for v in [3, 5, 8] {
            let info = StoryInfo::inspect(&story(v), "moon-house.z5").unwrap();
            assert_eq!(info.title, "moon house");
            assert_eq!(info.format(), format!("Z-machine v{v}"));
            assert!(info.compatibility().contains("Playable"));
            assert!(info
                .shelf_name("moon-house.z5")
                .starts_with("story-moon-house-"));
        }
    }
    #[test]
    fn recognized_but_unplayable_and_structurally_invalid_are_distinct() {
        assert_eq!(
            StoryInfo::inspect(b"Glulxxxx", "game.ulx"),
            Err(StoryError::Glulx)
        );
        let mut v6 = story(6);
        v6[0] = 6;
        assert_eq!(
            StoryInfo::inspect(&v6, "game.z6"),
            Err(StoryError::UnsupportedVersion(6))
        );
        let mut broken = story(5);
        broken[0x1a..0x1c].copy_from_slice(&999u16.to_be_bytes());
        assert_eq!(
            StoryInfo::inspect(&broken, "broken.z5"),
            Err(StoryError::Invalid("declared length exceeds the file"))
        );
    }
}
