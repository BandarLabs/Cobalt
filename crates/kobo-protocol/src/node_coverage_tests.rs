use super::*;
use kobo_ui::{BandAlign, BandSlot, Chip};

/// Every node kind, so the precomputed frame layout is checked against the
/// real encoder for all of them.
///
/// This exists because `Spacer` carried a stale length of nine bytes for
/// as long as it encoded an `i32`, and nothing noticed when it became a
/// single tag byte: no test had ever put a spacer through `encode`. The
/// encoder asserts its own predicted length in debug builds, so the bug was
/// one call away from being loud, and was silently over-reserving instead.
#[allow(clippy::too_many_lines, reason = "one literal per node variant")]
fn one_of_every_node() -> Vec<Node> {
    vec![
        Node::Table {
            id: NodeId(70),
            rows: vec![
                kobo_ui::TableRow {
                    header: true,
                    cells: vec!["Model".into(), "Top-1".into()],
                },
                kobo_ui::TableRow {
                    header: false,
                    cells: vec!["ResNet-50".into(), "76.1".into()],
                },
            ],
            weights: vec![240, 90],
        },
        Node::Heading {
            id: NodeId(1),
            text: "Heading".into(),
            level: 1,
        },
        Node::Text {
            id: NodeId(2),
            text: "Body".into(),
            links: Vec::new(),
        },
        Node::RichText {
            id: NodeId(52),
            text: "Styled body".into(),
            spans: vec![kobo_ui::RichTextSpan {
                start: 0,
                end: 6,
                presentation: kobo_ui::TextPresentation {
                    strong: true,
                    ..kobo_ui::TextPresentation::default()
                },
            }],
            links: Vec::new(),
            presentation: kobo_ui::ParagraphPresentation {
                alignment: kobo_ui::ParagraphAlignment::Center,
                line_height_percent: 130,
                ..kobo_ui::ParagraphPresentation::default()
            },
            selection: Some(kobo_ui::TextSelection {
                context: 3,
                offset: 11,
            }),
            formulae: vec![kobo_ui::InlineFormula {
                start: 7,
                end: 11,
                handle: PictureHandle(9),
                source: (40, 20),
            }],
        },
        Node::Quote {
            id: NodeId(30),
            depth: 2,
            role: kobo_ui::QuoteRole::Body,
            fold: None,
            text: "A reply".into(),
        },
        Node::Button {
            id: NodeId(3),
            action: ActionId(1),
            label: "Press".into(),
            state: ControlState::Disabled,
            emphasis: kobo_ui::Emphasis::Normal,
        },
        Node::Card {
            id: NodeId(4),
            children: vec![Node::Text {
                id: NodeId(5),
                text: "Nested".into(),
                links: Vec::new(),
            }],
        },
        Node::Divider { id: NodeId(6) },
        Node::Rows {
            id: NodeId(45),
            rows: vec![Row::new(
                ActionId(67),
                "Bleak House",
                "Charles Dickens",
                RowLead::Picture(
                    TilePicture {
                        handle: PictureHandle(9),
                        source: (190, 300),
                    },
                    Glyph::Book,
                ),
            )],
        },
        Node::Field {
            id: NodeId(40),
            action: ActionId(60),
            value: "dickens".into(),
            placeholder: "Search the library".into(),
            clear: Some(ActionId(61)),
            label: None,
            hint: None,
            error: None,
            kind: kobo_ui::FieldKind::Text,
        },
        Node::Field {
            id: NodeId(46),
            action: ActionId(68),
            value: "lip_token".into(),
            placeholder: "Token".into(),
            clear: None,
            label: Some("Lichess token".into()),
            hint: Some("From lichess.org, under API access tokens.".into()),
            error: Some("Lichess refused this token.".into()),
            kind: kobo_ui::FieldKind::Secret,
        },
        // Empty and with nothing to clear: both optional halves absent is
        // where a length mismatch hides.
        Node::Field {
            id: NodeId(41),
            action: ActionId(62),
            value: String::new(),
            placeholder: String::new(),
            clear: None,
            label: None,
            hint: None,
            error: None,
            kind: kobo_ui::FieldKind::Text,
        },
        Node::Chips {
            id: NodeId(42),
            chips: vec![
                Chip::new(ActionId(63), "Fiction").selected(true),
                Chip::new(ActionId(64), "History"),
            ],
        },
        Node::Chips {
            id: NodeId(43),
            chips: Vec::new(),
        },
        Node::Tabs {
            id: NodeId(44),
            tabs: vec![
                Chip::new(ActionId(65), "Discover"),
                Chip::new(ActionId(66), "Popular"),
            ],
            selected: 1,
        },
        Node::Facts {
            id: NodeId(31),
            entries: vec![
                ("Downloads".into(), "94,206".into()),
                ("Rights".into(), "Public domain in the USA".into()),
                // An empty value is legal and must survive the wire.
                ("Series".into(), String::new()),
            ],
        },
        // Every slot shape, because the width token is the part that
        // changes length on the wire and a fixed width is the only one
        // carrying a number after it.
        Node::Band {
            id: NodeId(25),
            align: BandAlign::Middle,
            slots: vec![
                BandSlot::fixed(
                    300,
                    vec![Node::Text {
                        id: NodeId(26),
                        text: "Cover".into(),
                        links: Vec::new(),
                    }],
                ),
                BandSlot::fill(vec![
                    Node::Heading {
                        id: NodeId(27),
                        text: "Moby Dick".into(),
                        level: 1,
                    },
                    Node::Secondary {
                        id: NodeId(28),
                        text: "Herman Melville".into(),
                    },
                ]),
                BandSlot::natural(vec![Node::Secondary {
                    id: NodeId(29),
                    text: "32".into(),
                }]),
            ],
        },
        // A slot holding nothing at all: the empty collection is where a
        // length mismatch hides.
        Node::Band {
            id: NodeId(30),
            align: BandAlign::Bottom,
            slots: vec![BandSlot::fill(Vec::new())],
        },
        // Both shapes of section: the value is the optional half, and an
        // optional half is exactly what a length mismatch hides in.
        Node::Section {
            id: NodeId(23),
            title: "Details".into(),
            value: None,
            link: None,
        },
        Node::Section {
            id: NodeId(24),
            title: "Popular".into(),
            value: Some("32".into()),
            link: Some(BarAction::new(ActionId(68), "View all ↗")),
        },
        Node::Flex { id: NodeId(90) },
        Node::Spacer {
            id: NodeId(7),
            space: Space::Medium,
        },
        Node::Progress {
            id: NodeId(8),
            value: Percent::new(40),
        },
        Node::Stepper {
            id: NodeId(96),
            label: "120%".into(),
            less: BarAction::new(ActionId(30), String::new()).with_glyph(Glyph::Minus),
            more: BarAction::new(ActionId(31), String::new()).with_glyph(Glyph::Plus),
            less_state: ControlState::Enabled,
            more_state: ControlState::Disabled,
            fill: Some(75),
        },
        Node::PagedList {
            id: NodeId(9),
            page: 0,
            items: vec!["one".into(), "two".into()],
        },
        Node::TileGrid {
            shape: TileShape::Square,
            id: NodeId(10),
            tiles: vec![
                Tile::new(ActionId(2), "Reader", Glyph::Reader),
                Tile::new(ActionId(3), "Notes", Glyph::Note),
                // A tile wearing every optional part at once, because the
                // empty ones are exactly where a length mismatch hides.
                Tile::new(ActionId(4), "Bleak House", Glyph::Book)
                    .with_state(TileState::Held)
                    .with_badge("12")
                    .with_subtitle("Charles Dickens")
                    .with_value("12"),
            ],
        },
        Node::PageRail {
            id: NodeId(91),
            page: 1,
            of: 3,
        },
        Node::Splash {
            id: NodeId(21),
            glyph: Some(Glyph::Book),
            title: "Gutenbird".into(),
            summary: "Sixty thousand free books.".into(),
        },
        // No mark, and nothing to say. Both halves are optional in
        // practice and the empty ones are what a length mismatch hides in.
        Node::Splash {
            id: NodeId(22),
            glyph: None,
            title: "Starting".into(),
            summary: String::new(),
        },
        Node::Rows {
            id: NodeId(20),
            rows: vec![
                Row::new(
                    ActionId(7),
                    "Hello",
                    "The smallest application.",
                    Glyph::App,
                ),
                // An empty summary is legal and must survive the wire.
                Row::new(ActionId(8), "Counter", "", Glyph::Note),
                // A trailing value is the optional half of a row, and an
                // optional half is where a length mismatch hides.
                Row::new(ActionId(9), "Great Expectations", "Dickens", Glyph::Book)
                    .with_trailing("18,204"),
                // An overflow action is the other optional half, and the
                // two together are where an ordering mistake hides.
                Row::new(ActionId(10), "Ars Technica", "arstechnica.com", Glyph::Rss)
                    .with_menu(ActionId(11)),
                Row::new(
                    ActionId(12),
                    "Hacker News",
                    "news.ycombinator.com",
                    Glyph::News,
                )
                .with_trailing("30")
                .with_menu(ActionId(13)),
            ],
        },
        Node::Choice {
            id: NodeId(11),
            prompt: "Pick one".into(),
            options: vec![
                BarAction::new(ActionId(4), "First"),
                BarAction::new(ActionId(5), "Second"),
            ],
            selected: Some(1),
            freeform: Some(Freeform::new(ActionId(6), "Something else")),
        },
        Node::Banner {
            id: NodeId(12),
            level: BannerLevel::Attention,
            text: "Battery low".into(),
            action: None,
        },
        Node::Banner {
            id: NodeId(18),
            level: BannerLevel::Error,
            text: "Lichess did not answer.".into(),
            action: Some(BarAction::new(ActionId(69), "Check again")),
        },
        Node::Banner {
            id: NodeId(19),
            level: BannerLevel::Success,
            text: "Signed in.".into(),
            action: None,
        },
        Node::Rows {
            id: NodeId(20),
            rows: vec![
                Row::new(ActionId(70), "Birds", "12 species", Glyph::Book)
                    .checked(true)
                    .with_badge(12)
                    .with_button(ActionId(71), "Open"),
                Row::new(ActionId(72), "Moths", "", Glyph::Book).checked(false),
                Row {
                    state: RowState::Mixed,
                    ..Row::new(ActionId(73), "Beetles", "", Glyph::Book)
                },
            ],
        },
        Node::Toggle {
            id: NodeId(21),
            action: ActionId(74),
            label: "Sleep".into(),
            summary: "After five minutes".into(),
            on: true,
            state: ControlState::Disabled,
        },
        Node::SelectionBar {
            id: NodeId(22),
            count: 3,
            actions: vec![
                BarAction::new(ActionId(75), "Delete"),
                BarAction::new(ActionId(76), "Move").with_glyph(Glyph::Folder),
            ],
        },
        Node::Options {
            id: NodeId(23),
            prompt: "Time zone".into(),
            options: vec![
                kobo_ui::OptionRow::new(ActionId(77), "London", "UTC+0"),
                kobo_ui::OptionRow::new(ActionId(78), "Paris", ""),
            ],
            selected: Some(1),
            page: 0,
            previous: ActionId(79),
            next: ActionId(80),
        },
        Node::Code {
            id: NodeId(24),
            payload: "https://lichess.org/device".into(),
            caption: "Scan to sign in".into(),
        },
        Node::Steps {
            id: NodeId(25),
            current: 1,
            total: 3,
            labels: vec!["Account".into(), "Sign in".into(), "Done".into()],
        },
        Node::Steps {
            id: NodeId(26),
            current: 0,
            total: 1,
            labels: Vec::new(),
        },
        Node::Skeleton {
            id: NodeId(13),
            lines: 3,
        },
        Node::Activity {
            id: NodeId(14),
            label: "Fetching articles".into(),
            progress: Some(Percent::new(45)),
            cancel: Some(BarAction::new(ActionId(7), "Cancel")),
            transferred: Some((4_404_019, Some(11_534_336))),
            failure: Some(TransferFailure {
                reason: "The connection was reset".into(),
                resumable: true,
            }),
        },
        Node::Activity {
            id: NodeId(15),
            label: "Connecting".into(),
            progress: None,
            cancel: None,
            transferred: None,
            failure: None,
        },
        Node::Activity {
            id: NodeId(56),
            label: "Downloading".into(),
            progress: None,
            cancel: None,
            transferred: Some((512, None)),
            failure: Some(TransferFailure {
                reason: "This edition is no longer published".into(),
                resumable: false,
            }),
        },
        Node::Terminal {
            id: NodeId(16),
            rows: vec!["~ # uname -a".into(), "Linux kobo 4.9.77".into()],
            cursor: Some(Caret::new(1, 17)),
        },
        Node::Terminal {
            id: NodeId(17),
            rows: Vec::new(),
            cursor: None,
        },
    ]
}

fn round_trip(screen: Screen) -> Screen {
    let frame = Frame {
        version: VERSION,
        request_id: 7,
        message: Message::SetScreen(screen),
    };
    let bytes = encode(&frame).expect("encode");
    match decode(&bytes).expect("decode").message {
        Message::SetScreen(screen) => screen,
        other => panic!("expected a screen, got {other:?}"),
    }
}

#[test]
fn numbered_boards_round_trip_without_changing_installed_grid_frames() {
    let mut cell = Cell::new(ActionId(11), "H").with_selected(true);
    cell.corner = Some(12);
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 1,
            square: true,
            cells: vec![cell],
        }],
    );
    assert_eq!(round_trip(screen.clone()), screen);
    let mut frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(screen),
    };
    let bytes = encode(&frame).unwrap();
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err());
    }
    frame.version = SELECTED_GRID_VERSION;
    assert!(encode(&frame).is_err());
    frame.version = VERSION;
    if let Message::SetScreen(s) = &mut frame.message {
        if let Node::Grid { cells, .. } = &mut s.nodes[0] {
            cells[0].corner = Some(100);
        }
    }
    assert!(encode(&frame).is_err());
}

#[test]
fn a_grid_cell_carries_its_glyph_across_the_wire() {
    // The cell payload gained a flag byte after its label. An encoder that
    // wrote it and a decoder that did not would read the flag as the top
    // byte of the next cell's action, so the second button of a transport
    // row would fire something nobody named. VERSION went to 3 for this.
    let cells = vec![
        kobo_ui::Cell::new(ActionId(11), "Back 30 sec")
            .with_glyph(Glyph::Rewind30)
            .with_selected(true),
        kobo_ui::Cell::new(ActionId(12), "Play"),
        kobo_ui::Cell::new(ActionId(13), "Louder").with_glyph(Glyph::VolumeUp),
    ];
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 3,
            square: false,
            cells: cells.clone(),
        }],
    );
    match round_trip(screen).nodes.first() {
        Some(Node::Grid { cells: back, .. }) => assert_eq!(back, &cells),
        other => panic!("expected a grid, got {other:?}"),
    }
}

#[test]
fn a_reading_screen_keeps_its_publisher_font_handle() {
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "Publisher prose".into(),
            links: Vec::new(),
        }],
    )
    .with_reading(true)
    .with_reading_font(Some(FontHandle(42)));
    assert_eq!(round_trip(screen.clone()), screen);
}

#[test]
fn a_folding_byline_survives_the_wire_with_its_count() {
    // The count is what the renderer draws beside the mark, so losing it
    // would show a bare plus with no idea how much is behind it -- and the
    // size table has to agree with the payload or every frame carrying one
    // fails the length check rather than the assertion here.
    for fold in [
        None,
        Some(kobo_ui::Fold {
            action: ActionId(4321),
            collapsed: true,
            hidden: 4095,
        }),
        Some(kobo_ui::Fold {
            action: ActionId(0),
            collapsed: false,
            hidden: 0,
        }),
    ] {
        let node = Node::Quote {
            id: NodeId(3),
            depth: 2,
            role: kobo_ui::QuoteRole::Byline,
            fold,
            text: "someone 3 hours ago".to_owned(),
        };
        assert_eq!(
            round_trip(Screen::new(1, vec![node.clone()])).nodes,
            vec![node.clone()],
            "a fold did not survive the wire: {fold:?}"
        );
    }
}

#[test]
fn an_overlay_survives_the_wire() {
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "Underneath".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_overlay(kobo_ui::Overlay::modal(
        NodeId(9),
        "Delete this?",
        vec![Node::Button {
            id: NodeId(10),
            action: ActionId(6),
            label: "Delete".to_owned(),
            state: ControlState::Enabled,
            emphasis: kobo_ui::Emphasis::Primary,
        }],
    ));
    assert_eq!(round_trip(screen.clone()).overlay, screen.overlay);
}

#[test]
fn every_node_kind_round_trips_byte_for_byte() {
    for node in one_of_every_node() {
        let screen = Screen::new(1, vec![node.clone()]);
        assert_eq!(
            round_trip(screen).nodes,
            vec![node.clone()],
            "node did not survive the wire: {node:?}"
        );
    }
}

#[test]
fn a_reply_deeper_than_the_cap_arrives_at_the_cap_rather_than_being_refused() {
    // Real discussion threads nest far past anything this panel can draw,
    // and forty levels is a deeper reply rather than a malformed frame. It
    // is clamped on the way out and again on the way in, so a peer that
    // never clamped cannot make the renderer indent off the panel.
    let screen = Screen::new(
        1,
        vec![Node::Quote {
            id: NodeId(1),
            depth: 40,
            role: kobo_ui::QuoteRole::Body,
            fold: None,
            text: "Deep in an argument".into(),
        }],
    );
    assert_eq!(
        round_trip(screen).nodes,
        vec![Node::Quote {
            id: NodeId(1),
            depth: kobo_ui::MAX_QUOTE_DEPTH,
            role: kobo_ui::QuoteRole::Body,
            fold: None,
            text: "Deep in an argument".into(),
        }]
    );
}

#[test]
fn a_screen_holding_every_node_round_trips() {
    let nodes = one_of_every_node();
    let screen = Screen::new(9, nodes.clone())
        .with_top_bar(TopBar::new(NodeId(100), "Gallery").action(ActionId(50), "Done"))
        .with_nav_bar(NavBar::new(
            NodeId(101),
            vec![
                BarAction::new(ActionId(60), "Home"),
                BarAction::new(ActionId(61), "Books"),
                BarAction::new(ActionId(62), "More"),
            ],
            Some(1),
        ));
    let decoded = round_trip(screen.clone());
    assert_eq!(decoded, screen);
}

#[test]
fn bars_survive_independently_of_each_other() {
    let only_top = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Title"));
    assert_eq!(round_trip(only_top.clone()), only_top);

    let only_nav = Screen::new(2, Vec::new()).with_nav_bar(NavBar::new(
        NodeId(2),
        vec![
            BarAction::new(ActionId(1), "A"),
            BarAction::new(ActionId(2), "B"),
        ],
        Some(0),
    ));
    assert_eq!(round_trip(only_nav.clone()), only_nav);

    let neither = Screen::new(3, Vec::new());
    assert_eq!(round_trip(neither.clone()), neither);
}

#[test]
fn the_reserved_back_action_cannot_arrive_from_an_application() {
    // Going back belongs to the runtime's navigation stack. If an app could
    // name that identifier it could draw a control the reader is entitled
    // to trust and then handle it however it liked.
    let screen = Screen::new(1, Vec::new())
        .with_top_bar(TopBar::new(NodeId(1), "Trap").action(ActionId::BACK, "Back"));
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(screen),
    };
    let bytes = encode(&frame).expect("encode");
    assert!(matches!(
        decode(&bytes),
        Err(ProtocolError::InvalidValue("reserved action id"))
    ));
}

/// A mark on a bar entry used to be a top bar privilege.
///
/// The flag was encoded beside the top bar rather than inside the shared
/// bar action, so a bottom bar could hold a glyph in memory, encode it,
/// and arrive on the panel as a word. The launcher is where it showed:
/// "Return to Kobo reader" in a slot a third of a panel wide.
#[test]
fn a_mark_on_any_bar_entry_survives_the_wire() {
    let screen = Screen::new(1, Vec::new())
        .with_nav_bar(NavBar::actions(
            NodeId(1),
            vec![
                BarAction::new(ActionId(1), "Previous").with_glyph(Glyph::Previous),
                BarAction::new(ActionId(2), "Reader"),
                BarAction::new(ActionId(3), "More apps").with_glyph(Glyph::Next),
            ],
        ))
        .with_top_bar(TopBar::new(NodeId(2), "Cobalt"));
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(screen.clone()),
    };
    let bytes = encode(&frame).expect("encode");
    // The reserved length and the encoder have to agree, and a mark that
    // is written but not counted is exactly how they stop agreeing.
    let counted = encoded_screen_len(&screen, 0, &mut 0, VERSION).expect("length");
    assert_eq!(bytes.len(), HEADER_LEN + counted);
    let Message::SetScreen(back) = decode(&bytes).expect("decode").message else {
        panic!("expected a screen");
    };
    let marks = back
        .nav_bar
        .expect("nav bar")
        .destinations
        .iter()
        .map(|destination| destination.glyph)
        .collect::<Vec<_>>();
    assert_eq!(
        marks,
        vec![Some(Glyph::Previous), None, Some(Glyph::Next)],
        "a bar entry came back without the mark it was given"
    );
}

/// A one-destination bar is refused by both halves, and the encoder
/// matters more than the decoder.
///
/// It used to encode cleanly and fail only on arrival. The runtime's
/// reader thread died on the malformed frame, the application never heard
/// about it, and it then waited forever on a socket nobody was reading:
/// the panel kept showing the previous screen and every later tap did
/// nothing at all. A Hacker News thread opening with a single "Stories"
/// destination is exactly how it was found.
#[test]
fn a_nav_bar_with_one_destination_is_rejected() {
    let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
        NodeId(1),
        vec![BarAction::new(ActionId(1), "Only")],
        Some(0),
    ));
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(screen),
    };
    assert!(matches!(
        encode(&frame),
        Err(ProtocolError::InvalidValue("nav bar destinations"))
    ));
}

/// The launcher and the library both meant "none of these is where you
/// are" and both said `usize::MAX`. The byte saturated to 255 and the
/// decoder clamped it onto the last destination, so both shipped with the
/// rightmost entry underlined on the panel, "More apps" on a launcher
/// showing page one, "Next" on a library showing the first page.
#[test]
fn no_destination_being_current_survives_the_wire() {
    let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
        NodeId(1),
        vec![
            BarAction::new(ActionId(1), "Back"),
            BarAction::new(ActionId(2), "Library"),
            BarAction::new(ActionId(3), "Next"),
        ],
        None,
    ));
    let decoded = round_trip(screen);
    assert_eq!(
        decoded.nav_bar.expect("nav bar").selected,
        None,
        "a bar of actions must not claim the reader is standing on one of them"
    );
}

#[test]
fn an_out_of_range_selection_clamps_rather_than_losing_navigation() {
    let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
        NodeId(1),
        vec![
            BarAction::new(ActionId(1), "A"),
            BarAction::new(ActionId(2), "B"),
        ],
        Some(250),
    ));
    let decoded = round_trip(screen);
    assert_eq!(decoded.nav_bar.expect("nav bar").selected, Some(1));
}

#[test]
fn an_answer_naming_no_option_arrives_unmarked_rather_than_refused() {
    let screen = Screen::new(
        1,
        vec![Node::Choice {
            id: NodeId(1),
            prompt: "Pick one".into(),
            options: vec![BarAction::new(ActionId(4), "First")],
            selected: Some(9),
            freeform: None,
        }],
    );
    let bytes = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(screen),
    })
    .expect("encode");
    let Message::SetScreen(screen) = decode(&bytes).expect("decode").message else {
        unreachable!("a set screen frame decodes as one")
    };
    let [Node::Choice { selected, .. }] = &screen.nodes[..] else {
        unreachable!("the screen is one choice")
    };
    assert_eq!(*selected, None);
}

#[test]
fn a_choice_offering_no_answers_is_rejected() {
    let screen = Screen::new(
        1,
        vec![Node::Choice {
            id: NodeId(1),
            prompt: "Dead end".into(),
            options: Vec::new(),
            selected: None,
            freeform: None,
        }],
    );
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(screen),
    };
    let bytes = encode(&frame).expect("encode");
    assert!(matches!(
        decode(&bytes),
        Err(ProtocolError::InvalidValue("choice with no answers"))
    ));
}

#[test]
fn unknown_tags_are_rejected_rather_than_guessed() {
    for (label, bytes) in [
        ("glyph", {
            let mut screen = Vec::new();
            let mut count = 0;
            encode_screen(
                &mut screen,
                &Screen::new(
                    1,
                    vec![Node::TileGrid {
                        shape: TileShape::Square,
                        id: NodeId(1),
                        tiles: vec![Tile::new(ActionId(1), "x", Glyph::App)],
                    }],
                ),
                0,
                &mut count,
                VERSION,
            )
            .expect("encode");
            let last = screen.len() - 1;
            screen[last] = 200;
            screen
        }),
        ("banner level", {
            let mut screen = Vec::new();
            let mut count = 0;
            encode_screen(
                &mut screen,
                &Screen::new(
                    1,
                    vec![Node::Banner {
                        id: NodeId(1),
                        level: BannerLevel::Info,
                        text: String::new(),
                        action: None,
                    }],
                ),
                0,
                &mut count,
                VERSION,
            )
            .expect("encode");
            // The level byte sits after the screen header, node tag and id,
            // ahead of the empty text's length and the action flag.
            let position = screen.len() - 4;
            screen[position] = 9;
            screen
        }),
    ] {
        let mut reader = Reader::new(&bytes);
        let mut count = 0;
        assert!(
            decode_screen(&mut reader, 0, &mut count, VERSION).is_err(),
            "an unknown {label} tag was accepted"
        );
    }
}
