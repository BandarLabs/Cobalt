use super::tests::PANELS;
use super::*;

fn heading(id: u32, text: &str) -> Node {
    Node::Heading {
        id: NodeId(id),
        text: text.to_owned(),
        level: 1,
    }
}

fn text(id: u32, words: &str) -> Node {
    Node::Text {
        id: NodeId(id),
        text: words.to_owned(),
        links: Vec::new(),
    }
}

fn command(id: u32, depth: u8, words: &str) -> Node {
    Node::Quote {
        id: NodeId(id),
        depth,
        role: QuoteRole::Command,
        text: words.to_owned(),
        fold: None,
    }
}

fn button(id: u32) -> Node {
    Node::Button {
        id: NodeId(id),
        action: ActionId(id),
        label: "Enter the address".to_owned(),
        state: ControlState::Enabled,
        emphasis: Emphasis::Primary,
    }
}

fn pairing() -> Screen {
    Screen::new(
        1,
        vec![
            heading(2, "Pair with your computer"),
            text(3, "On your computer, run these two."),
            heading(4, "If it cannot reach this reader"),
            text(5, "Enter the address and code it prints."),
            button(6),
        ],
    )
    .with_top_bar(TopBar::new(NodeId(90), "Deck"))
}

fn top_of(layout: &Layout, id: u32) -> Rect {
    layout
        .nodes
        .iter()
        .find(|node| node.id == NodeId(id))
        .map(|node| node.rect)
        .expect("laid out")
}

#[test]
fn protocol_seventeen_takes_the_heading_down_and_a_secondary_line_up() {
    let sizes = || {
        [
            FontSize::Caption,
            FontSize::Note,
            FontSize::Body,
            FontSize::Title,
            FontSize::Heading,
        ]
        .map(FontSize::tenth_mm)
    };
    assert_eq!(
        with_typography(Typography::Quiet, sizes),
        [24, 27, 31, 38, 46]
    );
    assert_eq!(
        with_typography(Typography::Folio, sizes),
        [24, 24, 31, 42, 54]
    );
    assert_eq!(
        with_typography(Typography::Paperback, sizes),
        [24, 24, 31, 42, 54]
    );
}

#[test]
fn a_screen_measured_in_folio_keeps_every_block_one_millimetre_from_the_next() {
    for (name, metrics) in PANELS {
        let layout = pairing()
            .with_typography(Typography::Folio)
            .layout_for(&metrics);
        let gap = metrics.space(Space::Tight);
        for (above, below) in [(2, 3), (3, 4), (4, 5), (5, 6)] {
            let above = top_of(&layout, above);
            assert_eq!(
                top_of(&layout, below).y,
                above.y + above.height + gap,
                "{name}"
            );
        }
        assert_eq!(layout.content.y, top_of(&layout, 2).y, "{name}");
    }
}

#[test]
fn a_heading_that_opens_a_second_section_is_set_four_millimetres_below_the_first() {
    for (name, metrics) in PANELS {
        let layout = pairing().layout_for(&metrics);
        let paragraph = top_of(&layout, 3);
        assert_eq!(
            top_of(&layout, 4).y,
            paragraph.y + paragraph.height + metrics.space(Space::Medium),
            "{name}"
        );
        // A heading belongs to what it introduces.
        let opening = top_of(&layout, 2);
        assert_eq!(
            top_of(&layout, 3).y,
            opening.y + opening.height + metrics.space(Space::Tight),
            "{name}"
        );
        let sentence = top_of(&layout, 5);
        assert_eq!(
            top_of(&layout, 6).y,
            sentence.y + sentence.height + metrics.space(Space::Small),
            "{name}"
        );
    }
}

#[test]
fn content_starts_three_millimetres_under_the_bar_except_on_a_page_of_a_book() {
    for (name, metrics) in PANELS {
        let quiet = pairing().layout_for(&metrics).content.y;
        let folio = pairing()
            .with_typography(Typography::Folio)
            .layout_for(&metrics)
            .content
            .y;
        assert_eq!(
            quiet - folio,
            metrics.tenth_mm(30) - metrics.space(Space::Tight),
            "{name}"
        );

        let mut book = pairing();
        book.reading = true;
        let mut folio_book = book.clone().with_typography(Typography::Folio);
        folio_book.reading = true;
        assert_eq!(
            book.layout_for(&metrics).content.y,
            folio_book.layout_for(&metrics).content.y,
            "{name}: a book page lost a line to the bar"
        );
    }
}

/// Pagination and layout have to agree on where the page starts, or the last
/// line a paginator fitted is drawn under the bottom margin.
#[test]
fn the_prose_area_starts_where_the_first_paragraph_is_drawn() {
    for (name, metrics) in PANELS {
        for reading in [false, true] {
            let mut screen = Screen::new(1, vec![text(2, "One paragraph.")])
                .with_top_bar(TopBar::new(NodeId(90), "Reader"));
            screen.reading = reading;
            let face = if reading { Face::Reading } else { Face::Text };
            let area = metrics.prose_area_in(true, false, face);
            let top = metrics.height - metrics.screen_margin() - area.height;
            assert_eq!(
                screen.layout_for(&metrics).content.y,
                top,
                "{name}, reading {reading}"
            );
        }
    }
}

#[test]
fn a_command_is_set_on_a_panel_the_width_of_the_column_whatever_its_depth() {
    for (name, metrics) in PANELS {
        let screen = Screen::new(
            1,
            vec![
                command(2, 3, "kobo sidekick init"),
                Node::Quote {
                    id: NodeId(3),
                    depth: 0,
                    role: QuoteRole::Body,
                    text: "A paragraph in the same column.".to_owned(),
                    fold: None,
                },
            ],
        );
        let layout = screen.layout_for(&metrics);
        let panel = layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(2))
            .expect("laid out");
        let column = top_of(&layout, 3);
        assert_eq!(
            panel.kind,
            LayoutKind::Quote(0, QuoteRole::Command),
            "{name}"
        );
        assert_eq!(
            (panel.rect.x, panel.rect.width),
            (column.x, column.width),
            "{name}"
        );
        assert_eq!(panel.text_lines, ["kobo sidekick init"], "{name}");
        let line = FontSize::Body.line_height_in(Face::Mono);
        assert_eq!(
            panel.rect.height,
            max(MIN_TEXT_HEIGHT, line + 2 * metrics.space(Space::Tight)),
            "{name}"
        );
        let issues = screen.validate(&metrics);
        assert!(issues.is_empty(), "{name}: {issues:?}");
    }
}

#[test]
fn a_command_wraps_at_its_spaces_and_never_after_a_slash_or_a_hyphen() {
    let words = "kobo frame push ~/Pictures/Family --device clara-bw";
    let (cell, _) = measure_text_in("0", FontSize::Body, Face::Mono);
    // Wider than the longest word, so every break has a space to fall at.
    for columns in 18..40 {
        let lines = wrap_command(words, cell * columns, FontSize::Body);
        let mut rejoined = lines
            .join(" ")
            .split(' ')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        rejoined.retain(|word| !word.is_empty());
        assert_eq!(
            rejoined,
            words.split(' ').collect::<Vec<_>>(),
            "{columns} columns broke a word: {lines:?}"
        );
        for line in &lines {
            assert!(
                measure_text_in(line, FontSize::Body, Face::Mono).0 <= cell * columns,
                "{line:?} is wider than {columns} columns"
            );
        }
    }
}

#[test]
fn a_word_wider_than_the_column_is_broken_between_characters_and_every_character_kept() {
    let path = "/Users/someone/Library/Application Support/Cobalt";
    let (cell, _) = measure_text_in("0", FontSize::Body, Face::Mono);
    let lines = wrap_command(path, cell * 8, FontSize::Body);
    assert_eq!(lines.concat().replace(' ', ""), path.replace(' ', ""));
    assert!(lines
        .iter()
        .all(|line| measure_text_in(line, FontSize::Body, Face::Mono).0 <= cell * 8));
    // A column narrower than one letter still ends.
    assert_eq!(wrap_command("abc", 1, FontSize::Body), ["a", "b", "c"]);
}

#[test]
fn a_command_reaching_a_paginated_thread_is_set_as_what_was_said() {
    let metrics = CLARA_BW_METRICS;
    let area = metrics.prose_area(true, false);
    let pages = paginate_tagged(
        &[(0, 0, QuoteRole::Command, "kobo connect hn")],
        &metrics,
        area,
    );
    assert_eq!(pages[0][0].2, QuoteRole::Body);
}

/// Wherever Folio fitted a screen, the quiet edition fits it too: the room it
/// adds is given back before a single line is lost for it.
#[test]
fn a_screen_with_no_room_to_spare_gives_the_room_back_before_it_loses_a_line() {
    let build = |words: usize| {
        Screen::new(
            1,
            vec![
                heading(2, "Pair with your computer"),
                text(3, &"word ".repeat(words)),
                button(4),
            ],
        )
        .with_top_bar(TopBar::new(NodeId(90), "Deck"))
    };
    for (name, metrics) in PANELS {
        let mut given_back = 0;
        let mut spaced = 0;
        for words in (20..4000).step_by(5) {
            let folio = build(words).with_typography(Typography::Folio);
            if !folio.validate(&metrics).is_empty() {
                break;
            }
            let quiet = build(words);
            let issues = quiet.validate(&metrics);
            assert!(issues.is_empty(), "{name}, {words} words: {issues:?}");
            if quiet.layout_for(&metrics).content.y == folio.layout_for(&metrics).content.y {
                given_back += 1;
            } else {
                spaced += 1;
            }
        }
        assert!(spaced > 0, "{name}: the room was never kept");
        assert!(given_back > 0, "{name}: the room was never needed back");
    }
}

/// A command that fits is drawn exactly as written: two spaces inside quotes
/// are part of an argument, and a line break the application wrote is kept.
#[test]
fn a_command_keeps_its_own_spaces_and_line_breaks() {
    let (cell, _) = measure_text_in("0", FontSize::Body, Face::Mono);
    assert_eq!(
        wrap_command(
            "printf '%s' 'a  b'\nkobo  sidekick run",
            cell * 40,
            FontSize::Body
        ),
        ["printf '%s' 'a  b'", "kobo  sidekick run"]
    );
    // Only the run of spaces a line breaks at is dropped.
    assert_eq!(
        wrap_command("kobo  frame push", cell * 11, FontSize::Body),
        ["kobo  frame", "push"]
    );
    assert_eq!(
        wrap_command("kobo frame  push\n", cell * 10, FontSize::Body),
        ["kobo frame", "push"]
    );
    assert_eq!(
        wrap_command("kobo deck init\n\nkobo deck run", cell * 20, FontSize::Body),
        ["kobo deck init", "", "kobo deck run"]
    );
}
