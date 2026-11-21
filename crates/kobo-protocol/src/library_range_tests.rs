use super::*;

fn frame(version: u8, message: Message) -> Frame {
    Frame {
        version,
        request_id: 4,
        message,
    }
}

fn ask(offset: u32, length: u32) -> Message {
    Message::DeviceRequest(DeviceRequest::ReadLibraryRange {
        id: "0/Atlas of the Coast.epub".to_owned(),
        offset,
        length,
    })
}

fn answer(offset: u32, size: u32, bytes: Vec<u8>) -> Message {
    Message::DeviceResult(DeviceResult::LibraryRange {
        id: "0/Atlas of the Coast.epub".to_owned(),
        offset,
        size,
        stamp: 1_763_600_000_000_000_000,
        bytes,
    })
}

/// The whole of a range answer's payload before its bytes, so a test can
/// reach the fields by where they are rather than by what they hold.
fn answer_fields(id: &str) -> usize {
    // The result tag, the identifier with its length, then offset, size,
    // stamp and the byte count.
    1 + 2 + id.len() + 4 + 4 + 8 + 4
}

#[test]
fn a_range_request_and_its_answer_round_trip_and_no_cut_frame_is_read() {
    let piece = u32::try_from(MAX_LIBRARY_RANGE_BYTES).unwrap();
    for message in [
        ask(0, piece),
        ask(u32::MAX, 1),
        answer(0, 3, b"PK\x03".to_vec()),
        // The last piece of a file: fewer bytes than a full range.
        answer(piece, piece + 5, b"tail!".to_vec()),
        // Asked for at the end, and past it once the file had shrunk.
        answer(40, 40, Vec::new()),
        answer(90, 40, Vec::new()),
    ] {
        let frame = frame(VERSION, message);
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err(), "cut at {length}");
        }
    }
}

#[test]
fn a_full_range_fits_one_frame_with_room_to_spare() {
    let piece = u32::try_from(MAX_LIBRARY_RANGE_BYTES).unwrap();
    let frame = frame(
        VERSION,
        answer(piece, 3 * piece, vec![0x5a; MAX_LIBRARY_RANGE_BYTES]),
    );
    let bytes = encode(&frame).expect("a full range is refused by the encoder");
    assert!(bytes.len() < MAX_FRAME_LEN / 8, "{} bytes", bytes.len());
    assert_eq!(decode(&bytes).expect("decode"), frame);
}

#[test]
fn a_range_of_nothing_or_of_more_than_one_piece_is_refused_at_both_ends() {
    let piece = u32::try_from(MAX_LIBRARY_RANGE_BYTES).unwrap();
    let good = encode(&frame(VERSION, ask(0, piece))).expect("encode");
    for length in [0, piece + 1, u32::MAX] {
        assert_eq!(
            encode(&frame(VERSION, ask(0, length))),
            Err(ProtocolError::InvalidValue("library range")),
            "{length} was encoded"
        );
        // The length is the request's last four bytes.
        let mut forged = good.clone();
        let end = forged.len();
        forged[end - 4..].copy_from_slice(&length.to_be_bytes());
        assert_eq!(
            decode(&forged),
            Err(ProtocolError::InvalidValue("library range")),
            "{length} was decoded"
        );
    }
}

#[test]
fn a_range_naming_no_library_document_is_refused_at_both_ends() {
    for id in [
        "",
        "/mnt/onboard/.kobo/KoboReader.sqlite",
        "0/../../etc/passwd",
        "0/Atlas\nof the Coast.epub",
    ] {
        for message in [
            Message::DeviceRequest(DeviceRequest::ReadLibraryRange {
                id: id.to_owned(),
                offset: 0,
                length: 1,
            }),
            Message::DeviceResult(DeviceResult::LibraryRange {
                id: id.to_owned(),
                offset: 0,
                size: 1,
                stamp: 0,
                bytes: vec![1],
            }),
        ] {
            assert!(encode(&frame(VERSION, message)).is_err(), "{id:?} encoded");
        }
    }
    // The decoder is asked separately, with a frame the encoder would never
    // have written: a good one whose identifier has been turned into a path.
    let id = "0/Atlas of the Coast.epub";
    for message in [ask(0, 1), answer(0, 1, vec![1])] {
        let mut forged = encode(&frame(VERSION, message)).expect("encode");
        // Past the header, the tag and the identifier's two length bytes.
        forged[HEADER_LEN + 3] = b'/';
        assert_eq!(forged[HEADER_LEN + 4], id.as_bytes()[1]);
        assert!(decode(&forged).is_err(), "an absolute path was decoded");
    }
}

#[test]
fn an_answer_whose_bytes_would_lie_outside_its_file_is_refused_at_both_ends() {
    for (offset, size, held) in [
        // One byte more than the file has left.
        (8, 10, 3),
        // Bytes from past the end of the file.
        (11, 10, 1),
        // An offset so large that adding to it would wrap.
        (u32::MAX, u32::MAX, 1),
    ] {
        assert_eq!(
            encode(&frame(VERSION, answer(offset, size, vec![7; held]))),
            Err(ProtocolError::InvalidValue("library range")),
            "{held} bytes at {offset} of {size} were encoded"
        );
    }
    assert_eq!(
        encode(&frame(
            VERSION,
            answer(0, u32::MAX, vec![7; MAX_LIBRARY_RANGE_BYTES + 1])
        )),
        Err(ProtocolError::InvalidValue("library range")),
        "more than one piece was encoded"
    );

    // A frame that says its file is shorter than the bytes it carries.
    let id = "0/Atlas of the Coast.epub";
    let good = encode(&frame(VERSION, answer(0, 4, vec![7; 4]))).expect("encode");
    let size_at = HEADER_LEN + 1 + 2 + id.len() + 4;
    let mut short = good.clone();
    short[size_at..size_at + 4].copy_from_slice(&3_u32.to_be_bytes());
    assert_eq!(
        decode(&short),
        Err(ProtocolError::InvalidValue("library range"))
    );

    // A frame that promises more bytes than it brought, and one that
    // promises more than a piece: neither count is believed far enough to
    // allocate for.
    let count_at = HEADER_LEN + answer_fields(id) - 4;
    for claimed in [5_u32, u32::MAX] {
        let mut long = good.clone();
        long[size_at..size_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        long[count_at..count_at + 4].copy_from_slice(&claimed.to_be_bytes());
        assert!(decode(&long).is_err(), "{claimed} bytes were believed");
    }
}

#[test]
fn ranges_are_refused_at_every_protocol_from_before_them() {
    // An application built for an earlier protocol has no reading for the
    // answer, and a runtime speaking one has none for the request.
    let request = encode(&frame(VERSION, ask(0, 1))).expect("encode");
    let result = encode(&frame(VERSION, answer(0, 1, vec![1]))).expect("encode");
    for version in ACCEPTED_VERSIONS
        .into_iter()
        .filter(|version| *version < LIBRARY_RANGE_VERSION)
    {
        for message in [ask(0, 1), answer(0, 1, vec![1])] {
            assert_eq!(
                encode(&frame(version, message)),
                Err(ProtocolError::UnsupportedVersion(version))
            );
        }
        for bytes in [&request, &result] {
            let mut older = bytes.clone();
            older[4] = version;
            assert!(decode(&older).is_err(), "protocol {version} read it");
        }
    }
}

#[test]
fn the_whole_document_request_still_carries_what_it_always_did() {
    // Books falls back to this under a runtime from before ranges, so its
    // bytes on the wire are a promise to that runtime.
    let bytes = encode(&frame(
        VERSION,
        Message::DeviceRequest(DeviceRequest::ReadLibrary {
            id: "0/a.txt".to_owned(),
        }),
    ))
    .expect("encode");
    assert_eq!(&bytes[HEADER_LEN..], b"\x31\x00\x070/a.txt");
    assert_eq!(MAX_LIBRARY_DOCUMENT_BYTES, 4 * 1024 * 1024);
}

/// A listing as full as the wire allows, of entries as long as it allows.
fn the_fullest_listing() -> Vec<LibraryEntry> {
    (0..MAX_LIBRARY_ENTRIES)
        .map(|place| {
            let name = format!("0/{place:04}/");
            LibraryEntry {
                id: format!("{name}{}", "a".repeat(MAX_LIBRARY_ID_LEN - name.len())),
                title: "t".repeat(MAX_LIBRARY_TITLE_LEN),
                kind: 1,
                bytes: u32::MAX,
                on_card: true,
            }
        })
        .collect()
}

#[test]
fn the_fullest_library_listing_fits_one_frame_many_times_over() {
    let frame = frame(
        VERSION,
        Message::DeviceResult(DeviceResult::Library {
            entries: the_fullest_listing(),
            truncated: true,
        }),
    );
    let bytes = encode(&frame).expect("a full listing is refused by the encoder");
    assert_eq!(bytes.len(), HEADER_LEN + 4 + MAX_LIBRARY_ENTRIES * 362);
    assert!(bytes.len() < MAX_FRAME_LEN / 10, "{} bytes", bytes.len());
    assert_eq!(decode(&bytes).expect("decode"), frame);
}

#[test]
fn a_listing_of_one_document_too_many_is_refused_at_both_ends() {
    let mut entries = the_fullest_listing();
    let full = encode(&frame(
        VERSION,
        Message::DeviceResult(DeviceResult::Library {
            entries: entries.clone(),
            truncated: false,
        }),
    ))
    .expect("encode");
    entries.push(entries[0].clone());
    assert_eq!(
        encode(&frame(
            VERSION,
            Message::DeviceResult(DeviceResult::Library {
                entries,
                truncated: false,
            }),
        )),
        Err(ProtocolError::InvalidValue("library listing"))
    );
    // The count is the two bytes after the result's tag.
    let mut forged = full;
    let count = u16::try_from(MAX_LIBRARY_ENTRIES + 1).expect("fits two bytes");
    forged[HEADER_LEN + 1..HEADER_LEN + 3].copy_from_slice(&count.to_be_bytes());
    assert_eq!(
        decode(&forged),
        Err(ProtocolError::InvalidValue("library listing"))
    );
}
