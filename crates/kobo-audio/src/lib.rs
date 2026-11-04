#![forbid(unsafe_code)]

//! Audiobooks as the runtime reads them: what a file is, what it says about
//! itself, and its sound as plain PCM a chunk at a time.
//!
//! Every container and codec here is pure Rust, so the same crate runs on the
//! reader, in the simulator and in the companion. A book is read as it is
//! played rather than decoded whole: the reader has half a gigabyte of memory
//! for everything, and a ten hour audiobook decoded to PCM is more than three
//! of them. What is held is a sixty-four kilobyte window on the file, the
//! decoder's own state, and the chunk that is about to be played.
//!
//! The pieces an application sees are a [`Description`] (the title, the
//! author, the chapters, the cover) and a [`Book`] that hands out sound. The
//! two DSP modules, [`resample`] and [`stretch`], turn that sound into what a
//! Bluetooth sink wants at the speed the listener chose.

pub mod resample;
pub mod stretch;

mod mp4;
mod opus;
mod tags;

use std::fmt;
use std::fs::File;
use std::io::{self, Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use symphonia::core::audio::{Channels, GenericAudioBufferRef};
use symphonia::core::codecs::audio::well_known as codec;
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as Symphonia;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::{MetadataOptions, RawValue, StandardTag};
use symphonia::core::units::{Time, TimeBase, Timestamp};

/// The most chapters a description carries. A book with more is listed with
/// its first two thousand; nothing on the device can draw more than that.
pub const MAX_CHAPTERS: usize = 2_000;
/// The longest chapter title kept, in bytes.
pub const MAX_CHAPTER_TITLE: usize = 160;
/// The largest cover handed on. Covers in audiobooks are often photographs
/// of a few megabytes; a panel of 758 by 1024 greys needs nothing like it.
pub const MAX_COVER_BYTES: usize = 2 * 1024 * 1024;
/// The most members an `.mp3z` archive or a folder may contribute as parts.
pub const MAX_PARTS: usize = 1_000;
/// The largest archive held in memory for its members. An `.mp3z` is read
/// whole because its members are read by name from the central directory;
/// a book made on the reader is a few megabytes.
pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

/// Every decoder this crate plays through: symphonia's own, and Opus from
/// the `opus-decoder` crate, registered once for the life of the process.
static CODECS: LazyLock<CodecRegistry> = LazyLock::new(|| {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder::<opus::Adapter>();
    registry
});

/// What went wrong with a book.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// The file is not there, or cannot be read.
    Unreadable,
    /// The file is a kind this crate does not play.
    Unsupported,
    /// The file is a kind this crate plays, and is not well formed.
    Damaged,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreadable => "the audio file could not be read",
            Self::Unsupported => "the audio file is a kind this reader cannot play",
            Self::Damaged => "the audio file is damaged",
        })
    }
}

impl std::error::Error for Fault {}

/// The kinds of file a book can be, told apart by name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Container {
    Mp3,
    /// MPEG-4 audio: `.m4b` and `.m4a`, usually AAC.
    Mp4,
    /// Ogg, carrying Vorbis.
    Ogg,
    /// Ogg carrying Opus.
    Opus,
    Flac,
    Wav,
    /// The firmware's own container for a sideloaded audiobook: a zip of
    /// MP3 parts.
    Mp3z,
}

impl Container {
    /// The container a file name names, if it names one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let lowered = name.to_ascii_lowercase();
        let suffix = lowered.rsplit('.').next()?;
        if !lowered.contains('.') {
            return None;
        }
        match suffix {
            "mp3" => Some(Self::Mp3),
            "m4b" | "m4a" | "mp4" | "aac" => Some(Self::Mp4),
            "ogg" | "oga" => Some(Self::Ogg),
            "opus" => Some(Self::Opus),
            "flac" => Some(Self::Flac),
            "wav" | "wave" => Some(Self::Wav),
            "mp3z" => Some(Self::Mp3z),
            _ => None,
        }
    }

    /// The short name a row shows.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Mp4 => "M4B",
            Self::Ogg => "OGG",
            Self::Opus => "OPUS",
            Self::Flac => "FLAC",
            Self::Wav => "WAV",
            Self::Mp3z => "MP3Z",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Mp3 | Self::Mp3z => "mp3",
            Self::Mp4 => "m4a",
            Self::Ogg | Self::Opus => "ogg",
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }
}

/// One chapter: where it starts and what it is called.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chapter {
    pub start_ms: u32,
    pub title: String,
}

/// What a book says about itself.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Description {
    pub title: Option<String>,
    pub author: Option<String>,
    pub narrator: Option<String>,
    pub album: Option<String>,
    /// In order of start, never empty for a book with parts.
    pub chapters: Vec<Chapter>,
    /// The cover as it was embedded, JPEG or PNG, under [`MAX_COVER_BYTES`].
    pub cover: Option<Vec<u8>>,
    /// The codec, its rate and its channels, as a short line: "AAC, 22 kHz mono".
    pub format: String,
    pub duration_ms: u32,
}

impl Description {
    /// The description as the wire carries it, each text cut to what the
    /// wire allows and the cover dropped when it is too large to send.
    #[must_use]
    pub fn to_wire(&self) -> kobo_protocol::AudioDetail {
        let text = |value: &Option<String>| {
            clip(
                value.as_deref().unwrap_or_default(),
                kobo_protocol::MAX_AUDIO_TEXT_LEN,
            )
        };
        kobo_protocol::AudioDetail {
            title: text(&self.title),
            author: text(&self.author),
            narrator: text(&self.narrator),
            album: text(&self.album),
            format: clip(&self.format, kobo_protocol::MAX_AUDIO_TEXT_LEN),
            duration_ms: self.duration_ms,
            chapters: self
                .chapters
                .iter()
                .take(kobo_protocol::MAX_AUDIO_CHAPTERS)
                .map(|chapter| kobo_protocol::AudioChapter {
                    start_ms: chapter.start_ms,
                    title: clip(&chapter.title, kobo_protocol::MAX_AUDIO_CHAPTER_TITLE_LEN),
                })
                .collect(),
            cover: self
                .cover
                .clone()
                .filter(|cover| cover.len() <= kobo_protocol::MAX_AUDIO_COVER_BYTES)
                .unwrap_or_default(),
        }
    }
}

/// Tags read from one file, before they are shaped into a description.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Tags {
    title: Option<String>,
    author: Option<String>,
    narrator: Option<String>,
    album: Option<String>,
    /// Kept apart from the narrator until [`Tags::settle`] decides which
    /// of them, if any, the narrator is.
    album_artist: Option<String>,
    composer: Option<String>,
    chapters: Vec<Chapter>,
    cover: Option<Vec<u8>>,
}

impl Tags {
    fn keep_chapter(&mut self, start_ms: u32, title: &str) {
        if self.chapters.len() >= MAX_CHAPTERS {
            return;
        }
        self.chapters.push(Chapter {
            start_ms,
            title: clip(title.trim(), MAX_CHAPTER_TITLE),
        });
    }

    fn keep_cover(&mut self, bytes: &[u8]) {
        if self.cover.is_none() && !bytes.is_empty() && bytes.len() <= MAX_COVER_BYTES {
            self.cover = Some(bytes.to_vec());
        }
    }
}

/// Where one part's bytes come from.
#[derive(Clone)]
enum Source {
    File(PathBuf),
    Memory(Arc<Vec<u8>>),
}

impl Source {
    fn open(&self) -> Result<Box<dyn MediaSource>, Fault> {
        match self {
            Self::File(path) => File::open(path)
                .map(|file| Box::new(file) as Box<dyn MediaSource>)
                .map_err(|_| Fault::Unreadable),
            Self::Memory(bytes) => Ok(Box::new(Cursor::new(Shared(Arc::clone(bytes))))),
        }
    }

    fn bytes(&self) -> u64 {
        match self {
            Self::File(path) => std::fs::metadata(path).map_or(0, |data| data.len()),
            Self::Memory(bytes) => u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }
    }
}

/// Bytes shared between the archive and the part that plays from it.
struct Shared(Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// One file of a book. A single-file book has one; an `.mp3z` or a folder of
/// files has one per member, in playing order.
struct Part {
    source: Source,
    container: Container,
    duration_ms: u32,
    /// What this part is called when it stands for a chapter.
    title: String,
    tags: Tags,
    format: String,
}

/// A part that is open for sound.
struct Playing {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: TimeBase,
    rate: u32,
    /// Decoded sound not yet handed out, interleaved stereo at `rate`.
    pending: Vec<i16>,
    /// How far into the part the next sample handed out is, at `rate`.
    position_frames: u64,
}

/// A book open for playing: one timeline over all its parts.
pub struct Book {
    parts: Vec<Part>,
    starts_ms: Vec<u32>,
    duration_ms: u32,
    description: Description,
    current: usize,
    playing: Option<Playing>,
    /// Set once the last part has run out.
    finished: bool,
}

/// A chunk of sound handed out by [`Book::read`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sound {
    /// Frames of interleaved stereo appended to the caller's buffer.
    pub frames: usize,
    /// The rate those frames are at. It changes only between parts.
    pub rate: u32,
}

impl Book {
    /// Opens one file. An `.mp3z` opens as its MP3 members in name order.
    ///
    /// # Errors
    /// The file is missing, is a kind this crate cannot play, or is damaged.
    pub fn open(path: &Path) -> Result<Self, Fault> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Fault::Unsupported)?;
        let container = Container::from_name(name).ok_or(Fault::Unsupported)?;
        if container == Container::Mp3z {
            let bytes = read_bounded(path, MAX_ARCHIVE_BYTES)?;
            return Self::open_archive(bytes, stem(name));
        }
        let title = stem(name);
        Ok(Self::from_parts(
            vec![part(Source::File(path.to_path_buf()), container, &title)?],
            None,
        ))
    }

    /// Opens bytes already in memory, named so their kind is known. An
    /// `.mp3z` is opened as its members.
    ///
    /// # Errors
    /// The name names no playable kind, or the bytes are damaged.
    pub fn open_bytes(bytes: Vec<u8>, name: &str) -> Result<Self, Fault> {
        let container = Container::from_name(name).ok_or(Fault::Unsupported)?;
        if container == Container::Mp3z {
            return Self::open_archive(bytes, stem(name));
        }
        let title = stem(name);
        Ok(Self::from_parts(
            vec![part(Source::Memory(Arc::new(bytes)), container, &title)?],
            None,
        ))
    }

    /// Opens a folder's files as one book called `title`, each file a chapter
    /// in the order given.
    ///
    /// A file that cannot be read or played fails the whole book. A book
    /// with a silent hole in the middle reads correctly until it skips a
    /// chapter, and nothing tells the listener that it did.
    ///
    /// # Errors
    /// No paths, too many, or any one of them unreadable, unplayable or
    /// damaged.
    pub fn open_parts(title: &str, paths: &[PathBuf]) -> Result<Self, Fault> {
        if paths.is_empty() || paths.len() > MAX_PARTS {
            return Err(Fault::Unsupported);
        }
        let mut parts = Vec::with_capacity(paths.len());
        for path in paths {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(Fault::Unsupported)?;
            let container = Container::from_name(name).ok_or(Fault::Unsupported)?;
            if container == Container::Mp3z {
                return Err(Fault::Unsupported);
            }
            parts.push(part(Source::File(path.clone()), container, &stem(name))?);
        }
        Ok(Self::from_parts(parts, Some(title)))
    }

    fn open_archive(bytes: Vec<u8>, title: String) -> Result<Self, Fault> {
        let archive = kobo_doc::zip::Archive::open(&bytes).map_err(|_| Fault::Damaged)?;
        let mut names = archive
            .names()
            .filter(|name| Container::from_name(name) == Some(Container::Mp3))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        names.sort();
        if names.is_empty() || names.len() > MAX_PARTS {
            return Err(Fault::Unsupported);
        }
        let mut parts = Vec::with_capacity(names.len());
        for name in &names {
            let member = archive.read(name).map_err(|_| Fault::Damaged)?;
            parts.push(part(
                Source::Memory(Arc::new(member)),
                Container::Mp3,
                &stem(name),
            )?);
        }
        drop(archive);
        drop(bytes);
        // One member is a book in a box; several are chapters of one.
        let title = (parts.len() > 1).then_some(title);
        Ok(Self::from_parts(parts, title.as_deref()))
    }

    fn from_parts(parts: Vec<Part>, title: Option<&str>) -> Self {
        let mut starts_ms = Vec::with_capacity(parts.len());
        let mut duration_ms = 0_u32;
        for part in &parts {
            starts_ms.push(duration_ms);
            duration_ms = duration_ms.saturating_add(part.duration_ms);
        }
        let description = describe(&parts, &starts_ms, duration_ms, title);
        Self {
            parts,
            starts_ms,
            duration_ms,
            description,
            current: 0,
            playing: None,
            finished: false,
        }
    }

    #[must_use]
    pub fn description(&self) -> &Description {
        &self.description
    }

    #[must_use]
    pub const fn duration_ms(&self) -> u32 {
        self.duration_ms
    }

    /// Where the next sound handed out comes from, in the book's timeline.
    #[must_use]
    pub fn position_ms(&self) -> u32 {
        let start = self.starts_ms.get(self.current).copied().unwrap_or(0);
        let within = self.playing.as_ref().map_or(0, |playing| {
            frames_to_ms(playing.position_frames, playing.rate)
        });
        start.saturating_add(within).min(self.duration_ms)
    }

    /// Whether the last part has run out.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Moves to `position_ms` in the book's timeline, between parts as
    /// needed. Lands at or shortly before the position asked for.
    ///
    /// # Errors
    /// The part holding the position cannot be opened or seeked.
    pub fn seek(&mut self, position_ms: u32) -> Result<(), Fault> {
        let wanted = position_ms.min(self.duration_ms);
        let index = self
            .starts_ms
            .iter()
            .rposition(|start| *start <= wanted)
            .unwrap_or(0);
        if index != self.current || self.playing.is_none() {
            self.current = index;
            self.playing = None;
            self.playing = Some(open_part(&self.parts[index])?);
        }
        self.finished = false;
        let within = wanted - self.starts_ms[index];
        let playing = self.playing.as_mut().ok_or(Fault::Damaged)?;
        playing.pending.clear();
        let time =
            Time::try_from_nanos_u128(u128::from(within) * 1_000_000).ok_or(Fault::Damaged)?;
        let seeked = playing
            .format
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time,
                    track_id: Some(playing.track_id),
                },
            )
            .map_err(|_| Fault::Damaged)?;
        playing.decoder.reset();
        let actual_ms = playing
            .time_base
            .calc_time(seeked.actual_ts)
            .map_or(within, time_ms);
        playing.position_frames = ms_to_frames(actual_ms, playing.rate);
        Ok(())
    }

    /// Appends up to `frames` frames of interleaved stereo to `out`, at the
    /// rate it reports. Fewer frames, or none, means the book is finished.
    ///
    /// # Errors
    /// A part cannot be opened, or its sound is damaged past recovery.
    pub fn read(&mut self, frames: usize, out: &mut Vec<i16>) -> Result<Sound, Fault> {
        if self.playing.is_none() {
            if self.finished || self.parts.is_empty() {
                return Ok(Sound {
                    frames: 0,
                    rate: 48_000,
                });
            }
            self.playing = Some(open_part(&self.parts[self.current])?);
        }
        let mut given = 0;
        let mut rate = None;
        loop {
            let playing = self.playing.as_mut().ok_or(Fault::Damaged)?;
            let rate = *rate.get_or_insert(playing.rate);
            if rate != playing.rate {
                // The next part plays at its own rate; this call ends at the
                // boundary so the caller can follow it.
                break;
            }
            if playing.pending.is_empty() && !fill(playing)? {
                // This part has run out. The next part plays at its own rate,
                // so this call ends here and the caller asks again.
                if self.current + 1 < self.parts.len() {
                    self.current += 1;
                    self.playing = None;
                } else {
                    self.finished = true;
                    self.playing = None;
                }
                break;
            }
            let take = (frames - given).min(playing.pending.len() / 2);
            out.extend_from_slice(&playing.pending[..take * 2]);
            playing.pending.drain(..take * 2);
            playing.position_frames = playing.position_frames.saturating_add(take as u64);
            given += take;
            if given >= frames {
                break;
            }
        }
        Ok(Sound {
            frames: given,
            rate: rate.unwrap_or(48_000),
        })
    }
}

/// Decodes packets until some sound is pending, or the part ends.
fn fill(playing: &mut Playing) -> Result<bool, Fault> {
    let mut native = Vec::new();
    loop {
        let packet = match playing.format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return Ok(false),
            Err(Symphonia::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(false);
            }
            Err(Symphonia::ResetRequired) => {
                playing.decoder.reset();
                continue;
            }
            Err(_) => return Err(Fault::Damaged),
        };
        if packet.track_id != playing.track_id {
            continue;
        }
        let decoded = match playing.decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // One bad frame is skipped, as every player skips it: a book
            // with a scratch in it is still a book.
            Err(Symphonia::DecodeError(_)) => continue,
            Err(Symphonia::ResetRequired) => {
                playing.decoder.reset();
                continue;
            }
            Err(_) => return Err(Fault::Damaged),
        };
        let rate = decoded.spec().rate();
        let channels = decoded.spec().channels().count();
        if rate == 0 || channels == 0 {
            continue;
        }
        if rate != playing.rate {
            // A stream that changes rate mid-part is not one this reader
            // follows; the pending sound is already at the old rate.
            return Err(Fault::Damaged);
        }
        native.clear();
        to_native(&decoded, &mut native);
        to_stereo(&native, channels, &mut playing.pending);
        if !playing.pending.is_empty() {
            return Ok(true);
        }
    }
}

fn to_native(decoded: &GenericAudioBufferRef<'_>, native: &mut Vec<i16>) {
    decoded.copy_to_vec_interleaved::<i16>(native);
}

/// Lays `channels`-wide frames out as stereo: a mono frame on both sides,
/// anything wider by its first two channels, which are the left and right
/// fronts in every layout this reader meets.
fn to_stereo(native: &[i16], channels: usize, stereo: &mut Vec<i16>) {
    for frame in native.chunks_exact(channels) {
        let left = frame[0];
        let right = frame.get(1).copied().unwrap_or(left);
        stereo.push(left);
        stereo.push(right);
    }
}

fn open_part(part: &Part) -> Result<Playing, Fault> {
    let source = part.source.open()?;
    let stream = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension(part.container.hint());
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(probe_fault)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(Fault::Unsupported)?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or(Fault::Unsupported)?;
    let decoder = CODECS
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|_| Fault::Unsupported)?;
    let rate = params
        .sample_rate
        .filter(|rate| *rate > 0)
        .ok_or(Fault::Damaged)?;
    let time_base = track
        .time_base
        .or_else(|| TimeBase::try_new(1, rate))
        .ok_or(Fault::Damaged)?;
    Ok(Playing {
        track_id: track.id,
        time_base,
        rate,
        format,
        decoder,
        pending: Vec::new(),
        position_frames: 0,
    })
}

/// What a failed probe means. The probe is only ever run on a file whose
/// name says it is a kind this crate plays, so a probe that finds no
/// container in it has found a file that is not what its name says: that
/// is damage, not a kind nobody plays. Only the decoders say unsupported.
fn probe_fault(error: Symphonia) -> Fault {
    match error {
        Symphonia::IoError(error) if error.kind() != io::ErrorKind::UnexpectedEof => {
            Fault::Unreadable
        }
        _ => Fault::Damaged,
    }
}

/// Reads what a file says about itself, and how long it plays, without
/// decoding any of its sound.
fn part(source: Source, container: Container, title: &str) -> Result<Part, Fault> {
    let (duration_ms, format, mut tags) = inspect(&source, container)?;
    if container == Container::Mp4 && tags.chapters.is_empty() {
        // The demuxer reads the MPEG-4 tags and cover but not its chapters,
        // which live in atoms of their own.
        mp4::read_chapters(&mut source.open()?, &mut tags);
    }
    let title = tags.title.clone().unwrap_or_else(|| title.to_owned());
    Ok(Part {
        source,
        container,
        duration_ms,
        title,
        tags,
        format,
    })
}

/// The duration, the format line and the tags, from the container's own
/// headers and without decoding any sound.
fn inspect(source: &Source, container: Container) -> Result<(u32, String, Tags), Fault> {
    let stream = MediaSourceStream::new(source.open()?, MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension(container.hint());
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(probe_fault)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(Fault::Unsupported)?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or(Fault::Unsupported)?;
    if CODECS.get_audio_decoder(params.codec).is_none() {
        return Err(Fault::Unsupported);
    }
    let rate = params.sample_rate.unwrap_or(0);
    let channels = params.channels.as_ref().map_or(0, Channels::count);
    let duration_ms = track
        .num_frames
        .zip(track.time_base)
        .and_then(|(frames, time_base)| {
            time_base.calc_time(Timestamp::new(i64::try_from(frames).ok()?))
        })
        .map_or(0, time_ms);
    let line = format_line(params.codec, rate, channels, source.bytes(), duration_ms);
    let tags = tags::read(&mut format);
    Ok((duration_ms, line, tags))
}

/// "AAC 32 kbps, 22 kHz mono": what the sound is, for a facts row.
fn format_line(
    codec: AudioCodecId,
    rate: u32,
    channels: usize,
    bytes: u64,
    duration_ms: u32,
) -> String {
    let name = match codec {
        codec::CODEC_ID_AAC => "AAC",
        codec::CODEC_ID_MP3 => "MP3",
        codec::CODEC_ID_VORBIS => "Vorbis",
        codec::CODEC_ID_FLAC => "FLAC",
        codec::CODEC_ID_OPUS => "Opus",
        codec::CODEC_ID_ALAC => "ALAC",
        _ => "PCM",
    };
    let mut line = name.to_owned();
    if duration_ms >= 1_000 && bytes > 0 && !matches!(codec, codec::CODEC_ID_FLAC) && name != "PCM"
    {
        let kbps = bytes
            .saturating_mul(8)
            .checked_div(u64::from(duration_ms))
            .unwrap_or(0);
        if kbps > 0 {
            line.push_str(&format!(" {kbps} kbps"));
        }
    }
    if rate > 0 {
        let khz = rate / 1_000;
        let tenths = rate % 1_000 / 100;
        if tenths == 0 {
            line.push_str(&format!(", {khz} kHz"));
        } else {
            line.push_str(&format!(", {khz}.{tenths} kHz"));
        }
    }
    match channels {
        0 => {}
        1 => line.push_str(" mono"),
        2 => line.push_str(" stereo"),
        count => line.push_str(&format!(" {count} channels")),
    }
    line
}

/// Shapes the parts' tags into one description of the book.
///
/// A single file is described by its own tags, and its chapters are the
/// ones inside it. A book of several parts is named by its folder or
/// archive, takes its author and cover from its first part, and has one
/// chapter per part, called what the part is called.
fn describe(
    parts: &[Part],
    starts_ms: &[u32],
    duration_ms: u32,
    title: Option<&str>,
) -> Description {
    let first = parts.first();
    let tags = first.map(|part| &part.tags);
    let mut chapters = Vec::new();
    if parts.len() > 1 {
        for (part, start) in parts.iter().zip(starts_ms) {
            if chapters.len() >= MAX_CHAPTERS {
                break;
            }
            chapters.push(Chapter {
                start_ms: *start,
                title: clip(&part.title, MAX_CHAPTER_TITLE),
            });
        }
    } else if let Some(tags) = tags {
        chapters.clone_from(&tags.chapters);
    }
    chapters.retain(|chapter| duration_ms == 0 || chapter.start_ms < duration_ms);
    chapters.sort_by_key(|chapter| chapter.start_ms);
    chapters.dedup_by_key(|chapter| chapter.start_ms);
    Description {
        title: title
            .map(str::to_owned)
            .or_else(|| tags.and_then(|tags| tags.title.clone()))
            .or_else(|| first.map(|part| part.title.clone())),
        author: tags.and_then(|tags| tags.author.clone()),
        narrator: tags.and_then(|tags| tags.narrator.clone()),
        album: tags.and_then(|tags| tags.album.clone()),
        chapters,
        cover: tags.and_then(|tags| tags.cover.clone()),
        format: first.map(|part| part.format.clone()).unwrap_or_default(),
        duration_ms,
    }
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, Fault> {
    let mut file = File::open(path).map_err(|_| Fault::Unreadable)?;
    let size = file.metadata().map_err(|_| Fault::Unreadable)?.len();
    if size > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(Fault::Unsupported);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|_| Fault::Unreadable)?;
    Ok(bytes)
}

/// Cuts `text` to at most `bytes` on a character boundary.
pub(crate) fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// A file name without its suffix, which is the name a part has when its
/// tags give it none.
fn stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
        _ => name.to_owned(),
    }
}

pub(crate) fn time_ms(time: Time) -> u32 {
    u32::try_from(time.as_nanos().max(0) / 1_000_000).unwrap_or(u32::MAX)
}

fn frames_to_ms(frames: u64, rate: u32) -> u32 {
    if rate == 0 {
        return 0;
    }
    u32::try_from(frames.saturating_mul(1_000) / u64::from(rate)).unwrap_or(u32::MAX)
}

fn ms_to_frames(ms: u32, rate: u32) -> u64 {
    u64::from(ms).saturating_mul(u64::from(rate)) / 1_000
}

/// A string tag's text, whatever symphonia read it as.
pub(crate) fn raw_text(value: &RawValue) -> Option<String> {
    match value {
        RawValue::String(text) => Some(text.as_str().to_owned()),
        RawValue::StringList(list) => list.first().cloned(),
        _ => None,
    }
}

/// The text of a standard tag that carries text.
pub(crate) fn standard_text(tag: &StandardTag) -> Option<(Which, String)> {
    let (which, text) = match tag {
        StandardTag::TrackTitle(text) => (Which::Title, text),
        StandardTag::Artist(text) | StandardTag::Author(text) => (Which::Author, text),
        StandardTag::AlbumArtist(text) => (Which::AlbumArtist, text),
        StandardTag::Album(text) => (Which::Album, text),
        StandardTag::Narrator(text) => (Which::Narrator, text),
        StandardTag::Composer(text) | StandardTag::Writer(text) => (Which::Composer, text),
        _ => return None,
    };
    Some((which, text.as_str().to_owned()))
}

/// The tags a description is shaped from, by what they mean rather than by
/// what each container calls them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Which {
    Title,
    Author,
    AlbumArtist,
    Album,
    Narrator,
    Composer,
}

impl Tags {
    /// Keeps the first tag of each meaning.
    pub(crate) fn keep(&mut self, which: Which, text: &str) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let slot = match which {
            Which::Title => &mut self.title,
            Which::Author => &mut self.author,
            Which::Album => &mut self.album,
            Which::Narrator => &mut self.narrator,
            Which::AlbumArtist => &mut self.album_artist,
            Which::Composer => &mut self.composer,
        };
        if slot.is_none() {
            *slot = Some(text);
        }
    }

    /// Decides who the narrator is.
    ///
    /// Audiobook tagging is not standardised: Apple's scheme puts the
    /// narrator in the composer, others in the album artist, and the
    /// narrator tag itself is rare. The artist is taken as the author
    /// because it is on every book. The narrator is the narrator tag, else
    /// the composer, else the album artist, and never a name that only
    /// repeats the author's: a byline that says "Ada Example, read by Ada
    /// Example" is a bug, not a fact.
    pub(crate) fn settle(&mut self) {
        let author = self.author.as_deref();
        let candidates = [
            self.narrator.take(),
            self.composer.take(),
            self.album_artist.take(),
        ];
        self.narrator = candidates
            .into_iter()
            .flatten()
            .find(|name| Some(name.as_str()) != author);
    }
}

/// Reads a `Seek + Read` source at a position, for the atom and tag parsers.
pub(crate) fn read_at<R: Read + Seek>(
    source: &mut R,
    offset: u64,
    length: usize,
) -> Option<Vec<u8>> {
    source.seek(io::SeekFrom::Start(offset)).ok()?;
    let mut bytes = vec![0; length];
    source.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests;
