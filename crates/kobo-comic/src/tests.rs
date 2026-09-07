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
