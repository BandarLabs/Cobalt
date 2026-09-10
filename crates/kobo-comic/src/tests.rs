use super::*;
use std::io::Write;
use zip::write::SimpleFileOptions;

fn fixture(entries: &[(&str, &[u8])], compression: CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(compression),
            )
            .expect("entry");
        writer.write_all(body).expect("body");
    }
    writer.finish().expect("archive").into_inner()
}

fn central(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .position(|part| part == b"PK\x01\x02")
        .expect("directory")
}

#[test]
fn pages_follow_numbers_and_nested_paths_and_ignore_metadata() {
    let bytes = fixture(
        &[
            ("volume2/10.jpg", &[1]),
            ("volume10/1.png", &[2]),
            ("volume2/2.JPG", &[3]),
            ("volume2/002.jpg", &[4]),
            ("ComicInfo.xml", b"metadata"),
            ("__MACOSX/._2.jpg", &[5]),
            (".cover.png", &[6]),
        ],
        CompressionMethod::Stored,
    );
    assert_eq!(
        inspect(&bytes).expect("comic").pages,
        [
            "volume2/002.jpg",
            "volume2/2.JPG",
            "volume2/10.jpg",
            "volume10/1.png"
        ]
    );
    assert_eq!(
        natural_order("9.png", "100000000000000000000000000000.png"),
        Ordering::Less
    );
}

#[test]
fn stored_and_deflated_pages_decode_from_original_images() {
    let png = kobo_image::encode_png_grey(2, 2, &[0, 64, 128, 255]).expect("original image");
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let bytes = fixture(&[("1.png", &png)], method);
        let comic = inspect(&bytes).expect("comic");
        let decoded = page(&bytes, &comic, 0).expect("page");
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        assert!(matches!(
            page(&bytes, &comic, 1),
            Err(ComicError::PageUnavailable)
        ));
    }
}

#[test]
fn rar_content_gets_conversion_guidance_regardless_of_extension() {
    for bytes in [&b"Rar!\x1a\x07\x00"[..], &b"Rar!\x1a\x07\x01\x00"[..]] {
        assert_eq!(inspect(bytes), Err(ComicError::CbrUnsupported));
    }
    assert!(ComicError::CbrUnsupported.to_string().contains("CBZ copy"));
    assert_eq!(inspect(b"not an archive"), Err(ComicError::UnknownFormat));
}

#[test]
fn metadata_is_bounded_optional_and_filename_never_overrides_content() {
    let png = kobo_image::encode_png_grey(2, 2, &[0, 64, 128, 255]).unwrap();
    let bytes = fixture(&[("2.png", &png), ("1.png", &png), ("ComicInfo.xml", b"<ComicInfo><Title>Rain</Title><Manga>YesAndRightToLeft</Manga><Pages><Page Image='1' Type='FrontCover'/></Pages></ComicInfo>")], CompressionMethod::Deflated);
    let (comic, notice) = inspect_named(&bytes, "volume.cbr").unwrap();
    assert_eq!(comic.pages, ["1.png", "2.png"]);
    assert_eq!(comic.metadata.title.as_deref(), Some("Rain"));
    assert_eq!(comic.metadata.cover, Some(1));
    assert!(notice.unwrap().contains(".cbz filename"));
    assert!(inspect_named(&bytes, "volume.CBZ").unwrap().1.is_none());
    let oversized = vec![b' '; 64 * 1024 + 1];
    for metadata in [&b"<ComicInfo><Title>Incomplete"[..], oversized.as_slice()] {
        let bytes = fixture(
            &[("1.png", &png), ("ComicInfo.xml", metadata)],
            CompressionMethod::Deflated,
        );
        let comic = inspect(&bytes).unwrap();
        assert!(comic.metadata.warning.is_some());
        assert!(page(&bytes, &comic, 0).is_ok());
    }
}

#[test]
fn source_crops_preserve_exact_pixels_and_reject_overflow() {
    let source =
        kobo_image::Picture::from_rgb(2, 2, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0])
            .unwrap();
    let cropped = source.crop(1, 0, 1, 2).unwrap();
    assert_eq!(cropped.colour(), Some(&[0, 255, 0, 255, 255, 0][..]));
    assert!(source.crop(u32::MAX, 0, 2, 2).is_err());
    assert!(source.crop(0, 0, 0, 1).is_err());
}

#[test]
fn shared_reader_restores_anchors_preferences_and_keeps_decoding_lazy() {
    use crate::{
        reader::Reader,
        viewport::{Fit, Viewport},
    };
    let png = kobo_image::encode_png_grey(2, 2, &[0, 64, 128, 255]).unwrap();
    let bytes = fixture(
        &[
            ("1.png", &png),
            ("2.png", &png),
            ("3.png", &png),
            ("4.png", b"not an image"),
        ],
        CompressionMethod::Deflated,
    );
    let mut reader = Reader::open(bytes.clone()).unwrap();
    assert_eq!(reader.cached_pages(), 0);
    assert!(reader.jump(1));
    reader.memory_mut().right_to_left = true;
    reader.memory_mut().viewport = Viewport::new(Fit::Width, 200, 7500, 10000);
    let saved = reader.memory().encode(reader.comic()).unwrap();
    for index in 0..3 {
        reader.thumbnail(index).unwrap();
        assert!(reader.cached_pages() <= 2);
    }
    assert_eq!(
        reader.memory().page,
        1,
        "previews must not move the bookmark"
    );
    let mut reopened = Reader::open(bytes).unwrap();
    reopened.restore(Some(&saved)).unwrap();
    assert_eq!(reopened.memory(), reader.memory());
    assert!(reopened.render((100, 200)).is_ok());
    assert!(reopened.jump(3));
    assert!(
        reopened.render((100, 200)).is_err(),
        "a damaged later page should fail when selected, not block earlier reading"
    );
    assert!(reopened.jump(2));
    assert!(reopened.render((100, 200)).is_ok());
    let prior = reopened.memory().clone();
    assert!(reopened.restore(Some(b"damaged")).is_err());
    assert_eq!(reopened.memory(), &prior);
    reopened.restore(Some(b"1")).unwrap();
    assert_eq!(
        reopened.memory().page,
        1,
        "migrate the previous Panels format"
    );
    let mut changed = reader.comic().clone();
    changed.pages.insert(0, "0.png".into());
    assert_eq!(
        crate::reader::Memory::restore(Some(&saved), &changed)
            .unwrap()
            .page,
        2
    );
}

#[test]
fn rejects_unsafe_names_even_when_the_entry_is_not_a_page() {
    for name in [
        "../secret.txt",
        "folder/../secret.txt",
        "/absolute.txt",
        "C:/comic.txt",
        "a\\b.txt",
        "./x.txt",
        "a//b.txt",
    ] {
        let bytes = fixture(&[(name, &[1]), ("1.png", &[2])], CompressionMethod::Stored);
        assert_eq!(inspect(&bytes), Err(ComicError::UnsafeEntry), "{name}");
    }
}

#[test]
fn duplicate_directory_names_cannot_be_hidden_by_the_zip_index() {
    let mut bytes = fixture(
        &[("1.jpg", &[1]), ("2.jpg", &[2])],
        CompressionMethod::Stored,
    );
    let first = central(&bytes);
    let second = first + 46 + 5;
    bytes[second + 46] = b'1';
    assert_eq!(inspect(&bytes), Err(ComicError::UnsafeEntry));
}

#[test]
fn links_encryption_and_unsupported_methods_are_refused() {
    let original = fixture(&[("1.png", &[1])], CompressionMethod::Stored);
    let offset = central(&original);
    let mut link = original.clone();
    link[offset + 5] = 3; // UNIX creator
    link[offset + 38..offset + 42].copy_from_slice(&(0o120_777_u32 << 16).to_le_bytes());
    assert_eq!(inspect(&link), Err(ComicError::UnsafeEntry));
    let mut encrypted = original.clone();
    encrypted[offset + 8] |= 1;
    assert!(matches!(
        inspect(&encrypted),
        Err(ComicError::Unsupported(_))
    ));
    let mut method = original;
    method[offset + 10..offset + 12].copy_from_slice(&12_u16.to_le_bytes());
    assert!(matches!(inspect(&method), Err(ComicError::Unsupported(_))));
}

#[test]
fn oversized_pages_and_directories_are_refused_before_decode() {
    let original = fixture(&[("1.png", &[1])], CompressionMethod::Stored);
    let mut bytes = original.clone();
    let offset = central(&bytes);
    bytes[offset + 24..offset + 28].copy_from_slice(
        &(u32::try_from(kobo_image::MAX_SOURCE_BYTES).expect("page limit") + 1).to_le_bytes(),
    );
    assert!(matches!(inspect(&bytes), Err(ComicError::Limit(_))));
    let mut bytes = original;
    let footer = bytes.len() - 22;
    bytes[footer + 8..footer + 12].copy_from_slice(&[1, 8, 1, 8]); // 2049 entries
    assert!(matches!(inspect(&bytes), Err(ComicError::Limit(_))));
}

#[test]
fn truncated_archives_and_crc_damage_are_not_silently_decoded() {
    let png = kobo_image::encode_png_grey(2, 2, &[0, 64, 128, 255]).expect("image");
    let mut bytes = fixture(&[("1.png", &png)], CompressionMethod::Stored);
    for length in 0..bytes.len() {
        assert!(
            inspect(&bytes[..length]).is_err(),
            "accepted prefix {length}"
        );
    }
    let comic = inspect(&bytes).expect("comic");
    let data = usize::try_from(
        ZipArchive::new(Cursor::new(&bytes))
            .expect("zip")
            .by_index(0)
            .expect("entry")
            .data_start(),
    )
    .expect("fixture offset");
    bytes[data] ^= 1;
    assert!(matches!(
        page(&bytes, &comic, 0),
        Err(ComicError::Archive(_))
    ));
}

#[test]
fn archive_inspection_is_lazy_but_invalid_images_fail_on_open() {
    let bytes = fixture(&[("1.png", b"not a picture")], CompressionMethod::Stored);
    let comic = inspect(&bytes).expect("metadata");
    assert!(matches!(page(&bytes, &comic, 0), Err(ComicError::Image(_))));
    let empty = fixture(&[("notes.txt", b"notes")], CompressionMethod::Stored);
    assert_eq!(inspect(&empty), Err(ComicError::Empty));
}

#[test]
fn spread_turns_keep_a_declared_cover_separate_on_both_sides() {
    let bytes = fixture(
        &[
            ("0.png", b"image"),
            ("1.png", b"image"),
            ("2.png", b"image"),
            ("3.png", b"image"),
            (
                "ComicInfo.xml",
                b"<ComicInfo><Pages><Page Image=\"1\" Type=\"FrontCover\"/></Pages></ComicInfo>",
            ),
        ],
        CompressionMethod::Stored,
    );
    let mut reader = crate::reader::Reader::open(bytes).unwrap();
    reader.memory_mut().spreads = true;
    let landscape = (300, 200);
    assert_eq!(reader.visible_pages(landscape), vec![0]);
    assert!(reader.turn(true, landscape));
    assert_eq!(reader.visible_pages(landscape), vec![1]);
    assert!(reader.turn(true, landscape));
    assert_eq!(reader.visible_pages(landscape), vec![2, 3]);
    assert!(reader.turn(false, landscape));
    assert_eq!(reader.visible_pages(landscape), vec![1]);
    assert!(reader.turn(false, landscape));
    assert_eq!(reader.visible_pages(landscape), vec![0]);
}

#[test]
fn colour_is_opt_in_and_survives_viewport_spreads_and_cache_mode_changes() {
    let rgb = kobo_image::encode_png_rgb(2, 2, &[200, 20, 40].repeat(4)).unwrap();
    let grey = kobo_image::encode_png_grey(2, 2, &[120; 4]).unwrap();
    let bytes = fixture(
        &[("1.png", &rgb), ("2.png", &rgb), ("3.png", &grey)],
        CompressionMethod::Deflated,
    );
    let mut reader = reader::Reader::open(bytes).unwrap();
    assert!(reader.render((2, 2)).unwrap().colour().is_none());
    reader.set_colour(true);
    assert_eq!(reader.cached_pages(), 0);
    assert_eq!(
        reader.render((2, 2)).unwrap().colour(),
        Some([200, 20, 40].repeat(4).as_slice())
    );
    reader.memory_mut().spreads = true;
    reader.jump(1);
    let spread = reader.render((4, 2)).unwrap();
    assert_eq!(&spread.colour().unwrap()[..6], &[200, 20, 40, 200, 20, 40]);
    assert_eq!(&spread.colour().unwrap()[6..12], &[120; 6]);
    reader.memory_mut().right_to_left = true;
    assert_eq!(
        &reader.render((4, 2)).unwrap().colour().unwrap()[..6],
        &[120; 6]
    );
    let position = reader.memory().clone();
    reader.set_colour(false);
    assert_eq!(reader.memory(), &position);
    assert!(reader.render((4, 2)).unwrap().colour().is_none());
}
