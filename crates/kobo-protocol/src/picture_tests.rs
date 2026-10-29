use super::*;

#[test]
fn a_picture_on_a_shelf_survives_the_wire() {
    let screen = Screen::new(
        9,
        vec![
            Node::TileGrid {
                id: NodeId(1),
                tiles: vec![
                    Tile::new(ActionId(11), "Waiting", Glyph::Book),
                    Tile::new(ActionId(12), "Moby Dick", Glyph::Book)
                        .with_picture(TilePicture::new(PictureHandle(7), 190, 300)),
                ],
                shape: TileShape::Portrait,
            },
            Node::Picture {
                id: NodeId(2),
                handle: PictureHandle(7),
                source: (190, 300),
                max_height_tenths_mm: 600,
                framed: true,
                bleed: false,
            },
        ],
    );
    let frame = Frame {
        version: VERSION,
        request_id: 3,
        message: Message::SetScreen(screen),
    };
    let bytes = encode(&frame).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), frame);
}

#[test]
fn handing_over_a_picture_survives_the_wire() {
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PutPicture {
            handle: PictureHandle(4),
            width: 3,
            height: 2,
            format: PictureFormat::Grey,
            pixels: vec![0, 32, 64, 96, 128, 160],
        },
    };
    let bytes = encode(&frame).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), frame);
}

/// Where the message tag sits in a frame: after the magic and the version.
const TAG_INDEX: usize = MAGIC.len() + 1;

#[test]
fn a_grey_picture_is_the_same_bytes_it_was_before_colour_existed() {
    // The frame an application built last year, byte for byte: magic,
    // version, request id, tag 18, length, then the handle, the size and
    // the pixels. A runtime from before colour reads it unchanged.
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PutPicture {
            handle: PictureHandle(4),
            width: 2,
            height: 1,
            format: PictureFormat::Grey,
            pixels: vec![7, 9],
        },
    };
    let bytes = encode(&frame).expect("encode");
    assert_eq!(bytes[MAGIC.len()], VERSION);
    assert_eq!(bytes[TAG_INDEX], 18, "the grey tag did not move");
    assert_eq!(
        &bytes[HEADER_LEN..],
        &[0, 0, 0, 4, 0, 0, 0, 2, 0, 0, 0, 1, 7, 9]
    );

    let begin = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::BeginPicture {
            handle: PictureHandle(4),
            width: 2,
            height: 1,
            format: PictureFormat::Grey,
        },
    })
    .expect("encode");
    assert_eq!(begin[TAG_INDEX], 20);
}

#[test]
fn a_colour_picture_travels_on_its_own_tag_at_three_bytes_a_pixel() {
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PutPicture {
            handle: PictureHandle(4),
            width: 2,
            height: 1,
            format: PictureFormat::Rgb,
            pixels: vec![255, 0, 0, 0, 0, 255],
        },
    };
    let bytes = encode(&frame).expect("encode");
    assert_eq!(bytes[TAG_INDEX], 28);
    assert_eq!(decode(&bytes).expect("decode"), frame);

    // The same pixels declared grey are the wrong length and refused; a
    // colour picture short of three bytes a pixel likewise.
    assert!(matches!(
        encode(&Frame {
            version: VERSION,
            request_id: 1,
            message: Message::PutPicture {
                handle: PictureHandle(4),
                width: 2,
                height: 1,
                format: PictureFormat::Grey,
                pixels: vec![255, 0, 0, 0, 0, 255],
            },
        }),
        Err(ProtocolError::InvalidValue(_))
    ));
    assert!(matches!(
        encode(&Frame {
            version: VERSION,
            request_id: 1,
            message: Message::PutPicture {
                handle: PictureHandle(4),
                width: 2,
                height: 1,
                format: PictureFormat::Rgb,
                pixels: vec![255, 0, 0, 0],
            },
        }),
        Err(ProtocolError::InvalidValue(_))
    ));

    let begin = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::BeginPicture {
            handle: PictureHandle(4),
            width: 1072,
            height: 1448,
            format: PictureFormat::Rgb,
        },
    };
    let bytes = encode(&begin).expect("a full colour panel fits the byte bound");
    assert_eq!(bytes[TAG_INDEX], 29);
    assert_eq!(decode(&bytes).expect("decode"), begin);
    // Two colour panels do not: the bound is on bytes, not pixels.
    assert!(matches!(
        encode(&Frame {
            version: VERSION,
            request_id: 1,
            message: Message::BeginPicture {
                handle: PictureHandle(4),
                width: 1072,
                height: 1448 * 2,
                format: PictureFormat::Rgb,
            },
        }),
        Err(ProtocolError::FrameTooLarge)
    ));
}

#[test]
fn a_picture_whose_size_disagrees_with_its_bytes_is_refused() {
    // The decoder allocates on the strength of the declared size, so the
    // two have to be checked against each other before anything is read.
    let refused = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PutPicture {
            handle: PictureHandle(4),
            width: 100,
            height: 100,
            format: PictureFormat::Grey,
            pixels: vec![0; 99],
        },
    });
    assert!(matches!(refused, Err(ProtocolError::InvalidValue(_))));
}

#[test]
fn a_picture_larger_than_a_frame_is_refused() {
    let refused = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PutPicture {
            handle: PictureHandle(4),
            width: u32::try_from(MAX_INLINE_PICTURE_BYTES + 1).expect("fits"),
            height: 1,
            format: PictureFormat::Grey,
            pixels: vec![0; MAX_INLINE_PICTURE_BYTES + 1],
        },
    });
    assert!(matches!(refused, Err(ProtocolError::FrameTooLarge)));
}

#[test]
fn every_phase_of_a_large_picture_upload_survives_the_wire() {
    let messages = [
        Message::BeginPicture {
            handle: PictureHandle(4),
            width: 1072,
            height: 1448,
            format: PictureFormat::Grey,
        },
        Message::PictureChunk {
            handle: PictureHandle(4),
            offset: 0,
            pixels: vec![17; 4096],
        },
        Message::CommitPicture {
            handle: PictureHandle(4),
        },
    ];
    for message in messages {
        let frame = Frame {
            version: VERSION,
            request_id: 1,
            message,
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }
}

#[test]
fn a_picture_chunk_is_independently_bounded() {
    let refused = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PictureChunk {
            handle: PictureHandle(4),
            offset: 0,
            pixels: vec![0; MAX_PICTURE_CHUNK_BYTES + 1],
        },
    });
    assert!(matches!(refused, Err(ProtocolError::FrameTooLarge)));
}

#[test]
fn releasing_a_picture_survives_the_wire() {
    let frame = Frame {
        version: VERSION,
        request_id: 8,
        message: Message::DropPicture {
            handle: PictureHandle(4),
        },
    };
    let bytes = encode(&frame).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), frame);
}

#[test]
fn a_publisher_font_travels_once_and_is_bounded() {
    let message = Message::PutFont {
        handle: FontHandle(7),
        name: "Book.otf".into(),
        bytes: b"OTTOfixture".to_vec(),
    };
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: message.clone(),
    };
    let bytes = encode(&frame).expect("encode font");
    assert_eq!(decode(&bytes).expect("decode font").message, message);

    let oversized = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::PutFont {
            handle: FontHandle(8),
            name: "huge.ttf".into(),
            bytes: vec![0; MAX_FONT_BYTES + 1],
        },
    };
    assert!(matches!(
        encode(&oversized),
        Err(ProtocolError::FrameTooLarge)
    ));
}

#[test]
fn an_unknown_tile_shape_is_refused_rather_than_guessed() {
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                tiles: Vec::new(),
                shape: TileShape::Square,
            }],
        )),
    };
    let mut bytes = encode(&frame).expect("encode");
    // An empty grid ends with its shape and then its tile count, and a
    // screen ends with whether it carries an overlay and then its accent, so
    // the shape is the fourth byte from the end.
    let shape = bytes.len() - 4;
    assert_eq!(bytes[shape], 0, "square");
    bytes[shape] = 9;
    assert!(matches!(
        decode(&bytes),
        Err(ProtocolError::InvalidValue("tile shape"))
    ));
}

#[test]
fn orientation_request_round_trips_and_keeps_portrait_as_the_default() {
    let frame = Frame {
        version: VERSION,
        request_id: 4,
        message: Message::SetOrientation(Orientation::Landscape),
    };
    assert_eq!(
        decode(&encode(&frame).expect("encode")).expect("decode"),
        frame
    );
    assert_eq!(Orientation::default(), Orientation::Portrait);
    assert_eq!(Orientation::Portrait as u8, 0);
}

#[test]
fn pre_folio_protocols_cannot_send_orientation_requests() {
    // Protocol 10 is read and answered as protocol 11 is, through the
    // same pre-Folio gates, so every gate is held for both.
    for version in crate::PRE_FOLIO_VERSIONS {
        let frame = Frame {
            version,
            request_id: 4,
            message: Message::SetOrientation(Orientation::Landscape),
        };
        assert_eq!(
            encode(&frame),
            Err(ProtocolError::InvalidValue("protocol 12 orientation"))
        );
    }
}

#[test]
fn a_basic_credential_survives_an_encode_decode_round_trip() {
    // A gated catalogue is reached by name, and the name is all an
    // application ever holds: the pair behind it is the runtime's.
    let work = Task::Post {
        url: "https://standardebooks.example/feeds/opds/all".to_owned(),
        body: String::new(),
        content_type: "application/x-www-form-urlencoded".to_owned(),
        credential: Some(Credential::basic("standardebooks")),
        headers: Vec::new(),
        max_bytes: 1024,
    };
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::Spawn {
            task: TaskId(1),
            work: work.clone(),
        },
    };
    let bytes = encode(&frame).expect("encodes");
    let Message::Spawn { work: back, .. } = decode(&bytes).expect("decodes").message else {
        panic!("not a spawn");
    };
    assert_eq!(back, work);
}
