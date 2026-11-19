//! What the demuxer learned about a file: its tags, its pictures and the
//! chapters it found in an `ID3v2` `CHAP` frame or a `CHAPTER001` comment.
//!
//! Symphonia reads `ID3v2`, Vorbis comments, FLAC pictures and the iTunes
//! `ilst` atom, and shapes the ones it recognises into standard tags. This
//! takes those, and the raw keys of the few it does not, and keeps the
//! first of each that is worth keeping.

use symphonia::core::formats::FormatReader;
use symphonia::core::meta::{ChapterGroup, ChapterGroupItem, StandardTag, StandardVisualKey};

use crate::{raw_text, time_ms, Tags, Which};

/// Reads the tags and chapters the demuxer holds for a file.
pub(crate) fn read(format: &mut Box<dyn FormatReader>) -> Tags {
    let mut tags = Tags::default();
    if let Some(chapters) = format.chapters() {
        flatten(chapters, &mut tags);
    }
    let mut metadata = format.metadata();
    let Some(revision) = metadata.skip_to_latest() else {
        return tags;
    };
    for tag in &revision.media.tags {
        if let Some((which, text)) = tag.std.as_ref().and_then(crate::standard_text) {
            tags.keep(which, &text);
            continue;
        }
        let Some(value) = raw_text(&tag.raw.value) else {
            continue;
        };
        match tag.raw.key.to_ascii_uppercase().as_str() {
            "TITLE" | "TIT2" => tags.keep(Which::Title, &value),
            "ARTIST" | "TPE1" => tags.keep(Which::Author, &value),
            "ALBUMARTIST" | "ALBUM ARTIST" | "PERFORMER" | "TPE2" => {
                tags.keep(Which::AlbumArtist, &value);
            }
            "ALBUM" | "TALB" => tags.keep(Which::Album, &value),
            "NARRATOR" | "\u{a9}NRT" => tags.keep(Which::Narrator, &value),
            "COMPOSER" | "TCOM" => tags.keep(Which::Composer, &value),
            _ => {}
        }
    }
    let front = revision
        .media
        .visuals
        .iter()
        .find(|visual| visual.usage == Some(StandardVisualKey::FrontCover));
    if let Some(visual) = front.or_else(|| revision.media.visuals.first()) {
        tags.keep_cover(&visual.data);
    }
    tags.settle();
    tags
}

/// Chapters in a group, and in the groups inside it, in the order given.
fn flatten(group: &ChapterGroup, tags: &mut Tags) {
    for item in &group.items {
        match item {
            ChapterGroupItem::Group(inner) => flatten(inner, tags),
            ChapterGroupItem::Chapter(chapter) => {
                let title = chapter
                    .tags
                    .iter()
                    .find_map(|tag| match &tag.std {
                        Some(StandardTag::ChapterTitle(text) | StandardTag::TrackTitle(text)) => {
                            Some(text.as_str().to_owned())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        chapter.tags.iter().find_map(|tag| {
                            matches!(
                                tag.raw.key.to_ascii_uppercase().as_str(),
                                "TITLE" | "TIT2" | "NAME"
                            )
                            .then(|| raw_text(&tag.raw.value))
                            .flatten()
                        })
                    })
                    .unwrap_or_default();
                tags.keep_chapter(time_ms(chapter.start_time), &title);
            }
        }
    }
}
