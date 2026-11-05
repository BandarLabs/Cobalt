//! The chapters of an MPEG-4 audio file, read from the `moov` atom without
//! touching the sound. The tags and the cover come from the demuxer, which
//! reads the `ilst` atom already; chapters are the one thing it leaves.
//!
//! Chapters come in two shapes, and a book made with ffmpeg carries both.
//! Nero's `chpl` atom under `udta` is a flat list of start times and names.
//! `QuickTime`'s is a text track that the audio track points at through
//! `tref/chap`, where each sample is one chapter's name and the sample table
//! gives its start. Apple's and Audible's tools write only the second, Nero's
//! and some converters only the first, so both are read and `chpl` wins when
//! both are present: it is the cheaper one to trust, being one atom rather
//! than four tables and a seek per chapter.
//!
//! The walk is streamed: an atom's header is read, and its body is read only
//! when it is one of the few atoms this cares about. The audio track's own
//! sample tables, which run to megabytes on a long book, are stepped over.

use std::io::{Read, Seek, SeekFrom};

use crate::{clip, read_at, Tags, MAX_CHAPTERS, MAX_CHAPTER_TITLE};

/// The largest `chpl` atom read into memory: two thousand chapters with
/// the longest names the atom can hold fit in a quarter of this.
const MAX_ATOM_BYTES: usize = 1024 * 1024;
/// The largest sample table read for a chapter track.
const MAX_TABLE_BYTES: usize = 64 * 1024;
/// The longest chapter sample read from a text track.
const MAX_SAMPLE_BYTES: usize = 1_024;
/// How deep the walk follows nested atoms.
const MAX_DEPTH: usize = 8;

/// One track's bookkeeping while its atoms are walked.
#[derive(Default)]
struct Track {
    id: u32,
    handler: [u8; 4],
    timescale: u32,
    /// The tracks this one points at as its chapters.
    chapters_in: Vec<u32>,
    /// Per-sample durations, expanded.
    durations: Vec<u32>,
    sizes: Vec<u32>,
    /// `(first chunk, samples per chunk)`, one based.
    chunk_runs: Vec<(u32, u32)>,
    chunk_offsets: Vec<u64>,
}

#[derive(Default)]
struct Found {
    /// Chapters from `chpl`, kept apart so they can win over the track's.
    nero: Vec<(u32, String)>,
    tracks: Vec<Track>,
}

/// Adds the chapters of an MPEG-4 file to `tags`, which has none yet.
pub(crate) fn read_chapters<R: Read + Seek>(source: &mut R, tags: &mut Tags) -> Option<()> {
    let end = source.seek(SeekFrom::End(0)).ok()?;
    let mut found = Found::default();
    walk(source, 0, end, 0, &mut found, &[]);
    if !found.nero.is_empty() {
        for (start_ms, title) in found.nero {
            tags.keep_chapter(start_ms, &title);
        }
    } else if let Some(track) = chapter_track(&found.tracks) {
        for (start_ms, title) in track_chapters(source, track) {
            tags.keep_chapter(start_ms, &title);
        }
    }
    Some(())
}

/// Walks the atoms between `from` and `to`, noting what matters.
fn walk<R: Read + Seek>(
    source: &mut R,
    from: u64,
    to: u64,
    depth: usize,
    found: &mut Found,
    path: &[[u8; 4]],
) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut at = from;
    while at + 8 <= to {
        let Some(header) = read_at(source, at, 8) else {
            return;
        };
        let mut size = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let kind = [header[4], header[5], header[6], header[7]];
        let mut body = at + 8;
        if size == 1 {
            let Some(large) = read_at(source, body, 8) else {
                return;
            };
            size = u64::from_be_bytes([
                large[0], large[1], large[2], large[3], large[4], large[5], large[6], large[7],
            ]);
            body += 8;
        } else if size == 0 {
            size = to - at;
        }
        if size < body - at || at + size > to {
            return;
        }
        let end = at + size;
        let mut below = path.to_vec();
        below.push(kind);
        match &kind {
            b"moov" | b"udta" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"tref" => {
                walk(source, body, end, depth + 1, found, &below);
            }
            b"chpl" if path.last() == Some(b"udta") => {
                if let Some(bytes) = bounded(source, body, end, MAX_ATOM_BYTES) {
                    found.nero = read_chpl(&bytes);
                }
            }
            b"tkhd" if path.last() == Some(b"trak") => {
                found.tracks.push(Track::default());
                if let Some(bytes) = bounded(source, body, end, MAX_TABLE_BYTES) {
                    if let Some(track) = found.tracks.last_mut() {
                        track.id = track_id(&bytes);
                    }
                }
            }
            b"chap" if path.last() == Some(b"tref") => {
                if let Some(bytes) = bounded(source, body, end, MAX_TABLE_BYTES) {
                    if let Some(track) = found.tracks.last_mut() {
                        track.chapters_in = bytes
                            .chunks_exact(4)
                            .map(|id| u32::from_be_bytes([id[0], id[1], id[2], id[3]]))
                            .collect();
                    }
                }
            }
            b"mdhd" if path.last() == Some(b"mdia") => {
                if let Some(bytes) = bounded(source, body, end, MAX_TABLE_BYTES) {
                    if let Some(track) = found.tracks.last_mut() {
                        track.timescale = timescale(&bytes);
                    }
                }
            }
            b"hdlr" if path.last() == Some(b"mdia") => {
                if let Some(bytes) = bounded(source, body, end, MAX_TABLE_BYTES) {
                    if let (Some(track), Some(handler)) =
                        (found.tracks.last_mut(), bytes.get(8..12))
                    {
                        track.handler = [handler[0], handler[1], handler[2], handler[3]];
                    }
                }
            }
            b"stts" | b"stsz" | b"stsc" | b"stco" | b"co64" if path.last() == Some(b"stbl") => {
                // Only a text track's tables are small enough to be worth
                // reading, and only a text track can be a chapter track.
                let text = found
                    .tracks
                    .last()
                    .is_some_and(|track| is_text(track.handler));
                if text {
                    if let Some(bytes) = bounded(source, body, end, MAX_TABLE_BYTES) {
                        if let Some(track) = found.tracks.last_mut() {
                            read_table(kind, &bytes, track);
                        }
                    }
                }
            }
            _ => {}
        }
        at = end;
    }
}

fn bounded<R: Read + Seek>(source: &mut R, from: u64, to: u64, limit: usize) -> Option<Vec<u8>> {
    let length = usize::try_from(to.checked_sub(from)?).ok()?;
    if length > limit {
        return None;
    }
    read_at(source, from, length)
}

fn is_text(handler: [u8; 4]) -> bool {
    matches!(&handler, b"text" | b"sbtl")
}

/// The track that holds the chapters: whichever one the audio points at, or
/// the only text track when nothing points.
fn chapter_track(tracks: &[Track]) -> Option<&Track> {
    let pointed = tracks
        .iter()
        .flat_map(|track| track.chapters_in.iter().copied())
        .find_map(|id| {
            tracks
                .iter()
                .find(|track| track.id == id && is_text(track.handler))
        });
    pointed.or_else(|| {
        let mut texts = tracks.iter().filter(|track| is_text(track.handler));
        let only = texts.next()?;
        texts.next().is_none().then_some(only)
    })
}

fn track_id(tkhd: &[u8]) -> u32 {
    let version = tkhd.first().copied().unwrap_or(0);
    let at = if version == 1 { 20 } else { 12 };
    tkhd.get(at..at + 4)
        .map_or(0, |id| u32::from_be_bytes([id[0], id[1], id[2], id[3]]))
}

fn timescale(mdhd: &[u8]) -> u32 {
    let version = mdhd.first().copied().unwrap_or(0);
    let at = if version == 1 { 20 } else { 12 };
    mdhd.get(at..at + 4).map_or(0, |scale| {
        u32::from_be_bytes([scale[0], scale[1], scale[2], scale[3]])
    })
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|word| u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
}

fn be64(bytes: &[u8], at: usize) -> Option<u64> {
    bytes.get(at..at + 8).map(|word| {
        u64::from_be_bytes([
            word[0], word[1], word[2], word[3], word[4], word[5], word[6], word[7],
        ])
    })
}

/// Reads one of a text track's sample tables. Every table is a full box:
/// four bytes of version and flags, a count, then the entries.
fn read_table(kind: [u8; 4], bytes: &[u8], track: &mut Track) {
    let Some(count) = be32(bytes, 4).and_then(|count| usize::try_from(count).ok()) else {
        return;
    };
    let count = count.min(MAX_CHAPTERS * 2);
    match &kind {
        b"stts" => {
            track.durations.clear();
            for entry in 0..count {
                let (Some(run), Some(duration)) =
                    (be32(bytes, 8 + entry * 8), be32(bytes, 12 + entry * 8))
                else {
                    break;
                };
                let run = usize::try_from(run).unwrap_or(0).min(MAX_CHAPTERS * 2);
                if track.durations.len() + run > MAX_CHAPTERS * 2 {
                    break;
                }
                track.durations.extend(std::iter::repeat_n(duration, run));
            }
        }
        b"stsz" => {
            track.sizes.clear();
            let fixed = be32(bytes, 4).unwrap_or(0);
            let Some(entries) = be32(bytes, 8).and_then(|count| usize::try_from(count).ok()) else {
                return;
            };
            let entries = entries.min(MAX_CHAPTERS * 2);
            if fixed != 0 {
                track.sizes = vec![fixed; entries];
            } else {
                for entry in 0..entries {
                    let Some(size) = be32(bytes, 12 + entry * 4) else {
                        break;
                    };
                    track.sizes.push(size);
                }
            }
        }
        b"stsc" => {
            track.chunk_runs.clear();
            for entry in 0..count {
                let (Some(first), Some(per_chunk)) =
                    (be32(bytes, 8 + entry * 12), be32(bytes, 12 + entry * 12))
                else {
                    break;
                };
                track.chunk_runs.push((first, per_chunk));
            }
        }
        b"stco" => {
            track.chunk_offsets.clear();
            for entry in 0..count {
                let Some(offset) = be32(bytes, 8 + entry * 4) else {
                    break;
                };
                track.chunk_offsets.push(u64::from(offset));
            }
        }
        b"co64" => {
            track.chunk_offsets.clear();
            for entry in 0..count {
                let Some(offset) = be64(bytes, 8 + entry * 8) else {
                    break;
                };
                track.chunk_offsets.push(offset);
            }
        }
        _ => {}
    }
}

/// The chapters a text track spells out: each sample's start, from the
/// durations before it, and its text.
fn track_chapters<R: Read + Seek>(source: &mut R, track: &Track) -> Vec<(u32, String)> {
    let mut chapters = Vec::new();
    if track.timescale == 0 || track.chunk_offsets.is_empty() || track.sizes.is_empty() {
        return chapters;
    }
    let offsets = sample_offsets(track);
    let mut elapsed = 0_u64;
    for (index, offset) in offsets.iter().enumerate().take(MAX_CHAPTERS) {
        let size = usize::try_from(track.sizes.get(index).copied().unwrap_or(0)).unwrap_or(0);
        let start_ms = u32::try_from(elapsed.saturating_mul(1_000) / u64::from(track.timescale))
            .unwrap_or(u32::MAX);
        elapsed =
            elapsed.saturating_add(u64::from(track.durations.get(index).copied().unwrap_or(0)));
        if !(2..=MAX_SAMPLE_BYTES).contains(&size) {
            continue;
        }
        let Some(sample) = read_at(source, *offset, size) else {
            continue;
        };
        let length = usize::from(u16::from_be_bytes([sample[0], sample[1]])).min(size - 2);
        let text = String::from_utf8_lossy(&sample[2..2 + length]);
        chapters.push((start_ms, clip(text.trim(), MAX_CHAPTER_TITLE)));
    }
    chapters
}

/// Where each sample of a track begins, from its chunk map.
fn sample_offsets(track: &Track) -> Vec<u64> {
    let mut offsets = Vec::new();
    let chunks = track.chunk_offsets.len();
    let mut sample = 0_usize;
    for (run, (first, per_chunk)) in track.chunk_runs.iter().enumerate() {
        let first = usize::try_from(first.saturating_sub(1)).unwrap_or(usize::MAX);
        let next_first = track.chunk_runs.get(run + 1).map_or(chunks, |(next, _)| {
            usize::try_from(next.saturating_sub(1)).unwrap_or(chunks)
        });
        let per_chunk = usize::try_from(*per_chunk).unwrap_or(0);
        for chunk in first..next_first.min(chunks) {
            let mut at = track.chunk_offsets[chunk];
            for _ in 0..per_chunk {
                if sample >= track.sizes.len() || offsets.len() >= MAX_CHAPTERS {
                    return offsets;
                }
                offsets.push(at);
                at = at.saturating_add(u64::from(track.sizes[sample]));
                sample += 1;
            }
        }
    }
    offsets
}

/// Nero chapters: a count, then for each a start in hundred-nanosecond
/// units and a short name.
fn read_chpl(bytes: &[u8]) -> Vec<(u32, String)> {
    let version = bytes.first().copied().unwrap_or(0);
    let mut at = if version == 0 { 4 } else { 8 };
    let Some(count) = bytes.get(at).copied() else {
        return Vec::new();
    };
    at += 1;
    let mut chapters = Vec::new();
    for _ in 0..count {
        let Some(start) = be64(bytes, at) else {
            break;
        };
        let Some(length) = bytes.get(at + 8).copied() else {
            break;
        };
        at += 9;
        let length = usize::from(length);
        let Some(name) = bytes.get(at..at + length) else {
            break;
        };
        at += length;
        let start_ms = u32::try_from(start / 10_000).unwrap_or(u32::MAX);
        chapters.push((
            start_ms,
            clip(String::from_utf8_lossy(name).trim(), MAX_CHAPTER_TITLE),
        ));
        if chapters.len() >= MAX_CHAPTERS {
            break;
        }
    }
    chapters
}
