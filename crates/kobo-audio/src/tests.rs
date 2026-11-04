use std::path::{Path, PathBuf};

use super::{Book, Container, Fault};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Decodes the whole book, returning every frame at the rate it came.
fn decode_all(book: &mut Book) -> (Vec<i16>, u32) {
    let mut out = Vec::new();
    let mut rate = 0;
    loop {
        let sound = book.read(1_024, &mut out).expect("sound");
        if sound.frames == 0 {
            break;
        }
        if rate == 0 {
            rate = sound.rate;
        }
    }
    (out, rate)
}

/// The pitch of a clean tone, from how often it crosses zero.
#[allow(clippy::cast_precision_loss, reason = "counts, not samples")]
fn pitch(samples: &[i16], rate: u32) -> f64 {
    let left = samples.iter().step_by(2).collect::<Vec<_>>();
    let crossings = left
        .windows(2)
        .filter(|pair| (*pair[0] < 0) != (*pair[1] < 0))
        .count();
    crossings as f64 * f64::from(rate) / left.len() as f64 / 2.0
}

#[test]
fn names_say_what_a_file_is() {
    assert_eq!(Container::from_name("Book.M4B"), Some(Container::Mp4));
    assert_eq!(Container::from_name("book.mp3"), Some(Container::Mp3));
    assert_eq!(Container::from_name("book.mp3z"), Some(Container::Mp3z));
    assert_eq!(Container::from_name("book.ogg"), Some(Container::Ogg));
    assert_eq!(Container::from_name("book.opus"), Some(Container::Opus));
    assert_eq!(Container::from_name("book.flac"), Some(Container::Flac));
    assert_eq!(Container::from_name("book.wav"), Some(Container::Wav));
    assert_eq!(Container::from_name("book.epub"), None);
    assert_eq!(Container::from_name("mp3"), None);
}

#[test]
fn an_m4b_tells_its_title_author_cover_and_nero_chapters() {
    let book = Book::open(&fixture("chapters.m4b")).expect("opens");
    let description = book.description();
    assert_eq!(description.title.as_deref(), Some("The Quiet Shelf"));
    assert_eq!(description.author.as_deref(), Some("Ada Example"));
    // The composer tag says "Read by Ada Example", which is the author's own
    // name once the words in front of it go, so there is no narrator to add.
    assert_eq!(description.narrator, None);
    assert!(description
        .cover
        .as_ref()
        .is_some_and(|cover| cover.starts_with(b"\x89PNG")));
    assert!(
        (5_900..=6_100).contains(&description.duration_ms),
        "{}",
        description.duration_ms
    );
    let chapters = description
        .chapters
        .iter()
        .map(|chapter| (chapter.start_ms, chapter.title.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        chapters,
        [
            (0, "One. A Familiar Light"),
            (2_000, "Two. A Slow History"),
            (4_000, "Three. A Quiet Future"),
        ]
    );
    assert!(
        description.format.starts_with("AAC"),
        "{}",
        description.format
    );
    assert!(
        description.format.contains("22 kHz mono"),
        "{}",
        description.format
    );
}

#[test]
fn an_m4b_with_only_a_chapter_track_still_has_chapters() {
    let book = Book::open(&fixture("track-chapters.m4b")).expect("opens");
    let chapters = book
        .description()
        .chapters
        .iter()
        .map(|chapter| (chapter.start_ms, chapter.title.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        chapters,
        [
            (0, "One. A Familiar Light"),
            (2_000, "Two. A Slow History"),
            (4_000, "Three. A Quiet Future"),
        ]
    );
}

#[test]
fn an_mp3_tells_its_id3_chapters_and_cover() {
    let book = Book::open(&fixture("chapters.mp3")).expect("opens");
    let description = book.description();
    assert_eq!(description.title.as_deref(), Some("Stereo Tone"));
    assert_eq!(description.author.as_deref(), Some("Ada Example"));
    assert_eq!(description.album.as_deref(), Some("Fixtures"));
    assert!(description
        .cover
        .as_ref()
        .is_some_and(|cover| cover.starts_with(b"\x89PNG")));
    let chapters = description
        .chapters
        .iter()
        .map(|chapter| (chapter.start_ms, chapter.title.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        chapters,
        [
            (0, "One. A Familiar Light"),
            (1_500, "Two. A Slow History"),
            (3_000, "Three. A Quiet Future"),
        ]
    );
    assert!(
        (3_900..=4_100).contains(&description.duration_ms),
        "{}",
        description.duration_ms
    );
    assert!(
        description.format.starts_with("MP3"),
        "{}",
        description.format
    );
}

#[test]
fn an_ogg_tells_its_comment_chapters() {
    let book = Book::open(&fixture("vorbis.ogg")).expect("opens");
    let description = book.description();
    assert_eq!(description.title.as_deref(), Some("Vorbis Tone"));
    assert_eq!(description.author.as_deref(), Some("Ada Example"));
    let chapters = description
        .chapters
        .iter()
        .map(|chapter| (chapter.start_ms, chapter.title.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        chapters,
        [(0, "One. A Familiar Light"), (2_000, "Two. A Slow History")]
    );
    assert!(
        description.format.starts_with("Vorbis"),
        "{}",
        description.format
    );
}

#[test]
fn every_format_decodes_to_the_tone_that_was_encoded() {
    for (name, hertz, rate) in [
        ("chapters.m4b", 440.0, 22_050),
        ("chapters.mp3", 330.0, 44_100),
        ("stereo.mp3", 330.0, 44_100),
        ("vorbis.ogg", 330.0, 22_050),
        ("opus.opus", 330.0, 48_000),
        ("pcm.wav", 220.0, 16_000),
        ("lossless.flac", 220.0, 44_100),
    ] {
        let mut book = Book::open(&fixture(name)).expect(name);
        let (samples, found_rate) = decode_all(&mut book);
        assert_eq!(found_rate, rate, "{name}");
        let seconds = u32::try_from(samples.len() / 2).expect("fits") / rate;
        let expected = book.duration_ms() / 1_000;
        let decoded_ms =
            u64::from(u32::try_from(samples.len() / 2).expect("fits")) * 1_000 / u64::from(rate);
        assert!(
            decoded_ms.abs_diff(u64::from(book.duration_ms())) < 150,
            "{name}: decoded {decoded_ms} ms of a {} ms book",
            book.duration_ms()
        );
        let _ = (seconds, expected);
        // The first tenth of a second is the encoder's own fade-in.
        let settled = &samples[rate as usize / 10 * 2..];
        let found = pitch(settled, rate);
        assert!(
            (found - hertz).abs() < 8.0,
            "{name}: {found} Hz, expected {hertz}"
        );
        assert!(book.is_finished(), "{name}");
    }
}

#[test]
fn seeking_lands_near_the_position_asked_for_in_every_format() {
    for name in [
        "chapters.m4b",
        "chapters.mp3",
        "vorbis.ogg",
        "opus.opus",
        "pcm.wav",
        "lossless.flac",
    ] {
        let mut book = Book::open(&fixture(name)).expect(name);
        let duration = book.duration_ms();
        let wanted = duration / 2;
        book.seek(wanted).expect(name);
        let landed = book.position_ms();
        assert!(
            landed <= wanted && wanted - landed < 400,
            "{name}: asked for {wanted} ms, landed at {landed} ms"
        );
        let mut out = Vec::new();
        let sound = book.read(512, &mut out).expect(name);
        assert_eq!(sound.frames, 512, "{name}");
        let after = book.position_ms();
        assert!(after > landed, "{name}: {after} after {landed}");
        // Seeking back to the start plays from the start.
        book.seek(0).expect(name);
        assert_eq!(book.position_ms(), 0, "{name}");
    }
}

#[test]
fn a_folder_of_files_is_one_book_with_a_chapter_per_file() {
    let mut book = Book::open_parts("Two Tones", &[fixture("pcm.wav"), fixture("lossless.flac")])
        .expect("opens");
    let description = book.description();
    assert_eq!(description.title.as_deref(), Some("Two Tones"));
    assert!(
        (3_900..=4_100).contains(&description.duration_ms),
        "{}",
        description.duration_ms
    );
    assert_eq!(
        description
            .chapters
            .iter()
            .map(|chapter| (chapter.start_ms, chapter.title.as_str()))
            .collect::<Vec<_>>(),
        [(0, "pcm"), (2_000, "lossless")]
    );
    // The first part plays at 16 kHz and the second at 44.1 kHz; a read
    // ends where the rate changes so the caller can follow it.
    let mut out = Vec::new();
    let mut rates = Vec::new();
    loop {
        let sound = book.read(100_000, &mut out).expect("sound");
        if sound.frames == 0 {
            break;
        }
        rates.push(sound.rate);
    }
    assert_eq!(rates, [16_000, 44_100]);
    // Seeking into the second part lands in it.
    book.seek(3_000).expect("seek");
    let landed = book.position_ms();
    assert!((2_600..=3_000).contains(&landed), "{landed}");
}

#[test]
fn an_mp3z_opens_as_its_members_in_name_order() {
    let member = std::fs::read(fixture("stereo.mp3")).expect("fixture");
    let archive = kobo_doc::zip::stored(&[
        ("002.mp3".to_owned(), member.clone()),
        ("001.mp3".to_owned(), member.clone()),
        ("notes.txt".to_owned(), b"not sound".to_vec()),
    ])
    .expect("archive");
    let book = Book::open_bytes(archive, "the-moon.mp3z").expect("opens");
    let description = book.description();
    assert_eq!(description.title.as_deref(), Some("the-moon"));
    assert_eq!(description.author.as_deref(), Some("Ada Example"));
    assert!(
        (7_800..=8_200).contains(&description.duration_ms),
        "{}",
        description.duration_ms
    );
    assert_eq!(description.chapters.len(), 2);
    assert_eq!(description.chapters[0].title, "Stereo Tone");
    assert_eq!(
        description.chapters[1].start_ms,
        description.duration_ms / 2
    );
}

#[test]
fn a_single_member_mp3z_is_the_book_inside_it() {
    let member = std::fs::read(fixture("chapters.mp3")).expect("fixture");
    let archive = kobo_doc::zip::stored(&[("001.mp3".to_owned(), member)]).expect("archive");
    let book = Book::open_bytes(archive, "the-quiet-shelf.mp3z").expect("opens");
    assert_eq!(book.description().title.as_deref(), Some("Stereo Tone"));
    assert_eq!(book.description().chapters.len(), 3);
}

#[test]
fn what_is_not_a_book_is_refused_by_name_or_by_content() {
    assert_eq!(
        Book::open(&fixture("cover.png")).err(),
        Some(Fault::Unsupported)
    );
    assert_eq!(
        Book::open(&fixture("missing.mp3")).err(),
        Some(Fault::Unreadable)
    );
    assert_eq!(
        Book::open_bytes(b"not an mp3 at all".to_vec(), "noise.mp3").err(),
        Some(Fault::Damaged)
    );
    assert_eq!(
        Book::open_bytes(b"PK\x03\x04garbage".to_vec(), "noise.mp3z").err(),
        Some(Fault::Damaged)
    );
}
