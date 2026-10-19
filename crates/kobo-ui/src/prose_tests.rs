use super::tests::PANELS;
use super::*;

#[test]
fn the_fallback_typesetter_treats_crlf_as_one_separator() {
    // Without a typesetter installed the fallback answers every wrap, and
    // breaking after the carriage return as well as after the line feed
    // put a blank line between every pair of lines in CRLF text.
    assert_eq!(
        fallback_line_breaks("a\r\nb"),
        vec![
            (3, BreakOpportunity::Mandatory),
            (4, BreakOpportunity::Mandatory)
        ]
    );
}

#[test]
fn a_book_with_windows_line_endings_still_has_paragraphs() {
    // Project Gutenberg serves CRLF. Splitting on "\n\n" alone matched
    // nothing, so an entire novel paginated as one paragraph: a solid wall
    // of words with no space anywhere in it.
    let area = CLARA_BW_METRICS.prose_area(true, false);
    let crlf = "First paragraph, which is short.\r\n\r\nSecond paragraph.\r\n\r\nThird one.";
    let unix = normalise_breaks(crlf);
    let from_crlf = paginate(crlf, area).concat();
    assert_eq!(
        from_crlf,
        paginate(&unix, area).concat(),
        "CRLF and LF paginated differently"
    );
    assert_eq!(
        from_crlf.len(),
        3,
        "paragraphs collapsed into {from_crlf:?}"
    );
    assert!(!from_crlf.iter().any(|line| line.contains('\r')));
}

const DESCRIPTION: &str = "It is a truth universally acknowledged, that a single man in \
        possession of a good fortune, must be in want of a wife.\n\nHowever little known the \
        feelings or views of such a man may be on his first entering a neighbourhood, this truth \
        is so well fixed in the minds of the surrounding families, that he is considered as the \
        rightful property of some one or other of their daughters.";

const DIALOGUE: &str = "\u{201c}My dear Mr. Bennet,\u{201d} said his lady to him one day, \
        \u{201c}have you heard that Netherfield Park is let at last?\u{201d}\n\nMr. Bennet \
        replied that he had not.\n\n\u{201c}But it is,\u{201d} returned she.\n\n\u{201c}Do you \
        not want to know who has taken it?\u{201d}\n\n\u{201c}You want to tell me, and I have no \
        objection to hearing it.\u{201d}\n\nThis was invitation enough.";

/// One comment, long enough to run past a page on its own.
///
/// Deliberately a single paragraph with no blank line anywhere in it,
/// because that is what a Hacker News reply is and what the old
/// pagination could not handle.
const LONG_REPLY: &str = "The thing nobody mentions about this approach is that it moves \
        the cost rather than removing it, and the place it moves the cost to is the one place \
        nobody is measuring. I ran into exactly this two years ago on a system an order of \
        magnitude smaller, and the failure looked like a performance problem for about a month \
        before anyone worked out that it was a correctness problem wearing a performance \
        problem as a coat. The short version is that the invariant everyone assumes holds at \
        the boundary does not hold once you have more than one writer, and every layer above \
        that boundary has quietly been relying on it. You can paper over it with a lock, and \
        that is what we did, and it worked, and then it stopped working the moment somebody \
        added a second process, because the lock was in the wrong address space. If you are \
        going to do this, do the boring thing first: write down what is actually guaranteed, \
        in one file, and make everything that depends on the guarantee say so out loud. It is \
        much less fun than the clever version and it is the only one I have seen survive a \
        year of other people editing it.";

fn book(source: &str, times: usize) -> String {
    vec![source; times].join("\n\n")
}

/// Lays a page out exactly as the runtime would and returns the bottom of
/// the lowest piece of text.
fn drawn(page: &[String], metrics: &DisplayMetrics) -> (usize, i32) {
    let nodes = page
        .iter()
        .enumerate()
        .map(|(index, paragraph)| Node::Text {
            id: NodeId(index as u32 + 1),
            text: paragraph.clone(),
            links: Vec::new(),
        })
        .collect();
    let screen = Screen::new(1, nodes)
        .with_top_bar(TopBar::new(NodeId(0), "A Book"))
        .with_nav_bar(NavBar::new(
            NodeId(900),
            vec![
                BarAction::new(ActionId(1), "Back"),
                BarAction::new(ActionId(2), "Library"),
                BarAction::new(ActionId(3), "Next"),
            ],
            None,
        ));
    let layout = screen.layout_with(metrics, &Chrome::with_back(true));
    let text = layout
        .nodes
        .iter()
        .filter(|node| node.kind == LayoutKind::Text)
        .collect::<Vec<_>>();
    let bottom = text
        .iter()
        .map(|node| node.rect.y + node.rect.height)
        .max()
        .unwrap_or(0);
    (text.len(), bottom)
}

#[test]
fn every_page_is_drawn_whole_on_every_panel() {
    // The layout engine stops at the bottom of the content area and drops
    // the rest, so a page that measured as fitting and does not is a page
    // whose last paragraph silently never appears.
    for (name, metrics) in PANELS {
        let area = metrics.prose_area(true, true);
        for (kind, source) in [("description", DESCRIPTION), ("dialogue", DIALOGUE)] {
            let pages = paginate(&book(source, 12), area);
            assert!(!pages.is_empty(), "{name} {kind} produced no pages");
            for (index, page) in pages.iter().enumerate() {
                let (shown, bottom) = drawn(page, &metrics);
                assert_eq!(
                    shown,
                    page.len(),
                    "{name} {kind} page {index}: {} of {} paragraphs were drawn",
                    shown,
                    page.len()
                );
                assert!(
                    bottom <= metrics.height - metrics.nav_bar_height(),
                    "{name} {kind} page {index} ran {} pixels under the page controls",
                    bottom - (metrics.height - metrics.nav_bar_height())
                );
            }
        }
    }
}

/// The same measurement as `drawn`, for a page that carries depth.
fn drawn_quoted(page: &[(u8, QuoteRole, String)], metrics: &DisplayMetrics) -> (usize, i32) {
    let nodes = page
        .iter()
        .enumerate()
        .map(|(index, (depth, role, paragraph))| Node::Quote {
            id: NodeId(index as u32 + 1),
            depth: *depth,
            role: *role,
            text: paragraph.clone(),
            fold: None,
        })
        .collect();
    let screen = Screen::new(1, nodes)
        .with_top_bar(TopBar::new(NodeId(0), "A Thread"))
        .with_nav_bar(NavBar::new(
            NodeId(900),
            vec![
                BarAction::new(ActionId(1), "Back"),
                BarAction::new(ActionId(2), "Stories"),
                BarAction::new(ActionId(3), "Next"),
            ],
            None,
        ));
    let layout = screen.layout_with(metrics, &Chrome::with_back(true));
    let quotes = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Quote(..)))
        .collect::<Vec<_>>();
    let bottom = quotes
        .iter()
        .map(|node| node.rect.y + node.rect.height)
        .max()
        .unwrap_or(0);
    (quotes.len(), bottom)
}

#[test]
fn every_page_of_a_thread_is_drawn_whole_on_every_panel() {
    // An indented paragraph is narrower, so it wraps to more lines and
    // takes more of the page. Paginating a thread flat and drawing it
    // indented loses the bottom of nearly every page, and the layout
    // engine reports nothing when it does.
    for (name, metrics) in PANELS {
        let area = metrics.prose_area(true, true);
        let source = book(DIALOGUE, 12);
        let paragraphs = source
            .split("\n\n")
            .enumerate()
            .map(|(index, paragraph)| ((index % 5) as u8, QuoteRole::Body, paragraph))
            .collect::<Vec<_>>();
        let pages = paginate_quoted(&paragraphs, &metrics, area);
        assert!(!pages.is_empty(), "{name} produced no pages");
        for (index, page) in pages.iter().enumerate() {
            let (shown, bottom) = drawn_quoted(page, &metrics);
            assert_eq!(
                shown,
                page.len(),
                "{name} page {index}: {shown} of {} paragraphs were drawn",
                page.len()
            );
            assert!(
                bottom <= metrics.height - metrics.nav_bar_height(),
                "{name} page {index} ran {} pixels under the page controls",
                bottom - (metrics.height - metrics.nav_bar_height())
            );
        }
    }
}

#[test]
fn a_long_comment_fills_the_page_it_starts_on() {
    // A threaded discussion is paragraphs of wildly different lengths, and
    // one comment is one paragraph. Moving a paragraph whole to the next
    // page rather than splitting it meant a five-hundred-word reply left
    // most of the previous page blank, so a thread took twice the page
    // turns it needed. Both sides of a split keep at least two lines.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let line_height = FontSize::Body.line_height();
    let per_page = (area.height / line_height) as usize;
    // Two of them, still as one paragraph: a reply this long is ordinary
    // on a thread about anything contentious.
    let reply = [LONG_REPLY; 2].join(" ");
    let paragraphs = vec![
        (0_u8, QuoteRole::Body, "A short opening remark."),
        (1, QuoteRole::Body, reply.as_str()),
    ];
    let pages = paginate_quoted(&paragraphs, &CLARA_BW_METRICS, area);
    assert!(pages.len() > 1, "the reply was not long enough to split");
    assert_eq!(
        pages[0].len(),
        2,
        "the reply did not start on the first page"
    );
    let (_, bottom) = drawn_quoted(&pages[0], &CLARA_BW_METRICS);
    let slack = (area.height + CLARA_BW_METRICS.screen_margin()) - bottom;
    assert!(
        slack < line_height * 3,
        "the first page left {slack} pixels empty, about {} lines",
        slack / line_height
    );
    for (index, page) in pages.iter().enumerate() {
        let lines = page
            .iter()
            .map(|(depth, _, text)| {
                let (_, width) = quote_offsets(&CLARA_BW_METRICS, area.width, *depth);
                wrap_text(text, width, FontSize::Body).len()
            })
            .sum::<usize>();
        assert!(
            lines <= per_page,
            "page {index} carries {lines} lines into room for {per_page}"
        );
    }
    let last = pages.last().expect("pages");
    let (depth, _, text) = last.last().expect("a paragraph");
    let (_, width) = quote_offsets(&CLARA_BW_METRICS, area.width, *depth);
    assert!(
        wrap_text(text, width, FontSize::Body).len() >= MIN_KEEP_LINES,
        "the split left an orphan line alone on the last page"
    );
}

#[test]
fn a_thread_holds_less_per_page_than_the_same_words_unindented() {
    // If this ever stops being true, indentation is not being measured and
    // the previous test is passing by luck.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let source = book(DESCRIPTION, 60);
    let flat = source
        .split("\n\n")
        .map(|paragraph| (0, QuoteRole::Body, paragraph))
        .collect::<Vec<_>>();
    let nested = source
        .split("\n\n")
        .map(|paragraph| (3, QuoteRole::Body, paragraph))
        .collect::<Vec<_>>();
    let flat_pages = paginate_quoted(&flat, &CLARA_BW_METRICS, area).len();
    let nested_pages = paginate_quoted(&nested, &CLARA_BW_METRICS, area).len();
    assert!(
        nested_pages > flat_pages,
        "the same words took {nested_pages} pages indented and {flat_pages} flat"
    );
}

#[test]
fn a_reply_is_set_in_from_what_it_answers_and_stops_at_the_cap() {
    // Depth past the cap shares the deepest indent rather than marching
    // off the panel: at forty levels there would be no measure left.
    let nodes = (0..6)
        .map(|depth| Node::Quote {
            id: NodeId(depth + 1),
            depth: depth as u8,
            role: QuoteRole::Body,
            fold: None,
            text: "A reply, which answers the one above it.".to_owned(),
        })
        .collect();
    let layout = Screen::new(1, nodes).layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let quotes = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Quote(..)))
        .collect::<Vec<_>>();
    assert_eq!(quotes.len(), 6);
    for pair in quotes.windows(2) {
        let (shallow, deep) = (pair[0], pair[1]);
        let capped = matches!(deep.kind, LayoutKind::Quote(depth, _) if depth == MAX_QUOTE_DEPTH)
            && matches!(shallow.kind, LayoutKind::Quote(depth, _) if depth == MAX_QUOTE_DEPTH);
        if capped {
            assert_eq!(shallow.rect.x, deep.rect.x, "past the cap the indent moved");
            assert_eq!(shallow.rect.width, deep.rect.width);
        } else {
            assert!(
                deep.rect.x > shallow.rect.x,
                "a reply at depth {:?} did not start further in than {:?}",
                deep.kind,
                shallow.kind
            );
            assert!(
                deep.rect.width < shallow.rect.width,
                "a deeper reply was not narrower"
            );
        }
    }
    let widest = quotes[0].rect.x;
    let deepest = quotes[5].rect.x;
    assert!(
        deepest - widest < CLARA_BW_METRICS.prose_area(false, false).width / 3,
        "the deepest indent spent more than a third of the measure"
    );
}

/// Every pixel of a given tone inside a rect.
fn tone_count(surface: &Surface, rect: Rect, want: u8, stride: usize) -> usize {
    let mut found = 0;
    for row in rect.y..rect.y + rect.height {
        for column in rect.x..rect.x + rect.width {
            let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
                continue;
            };
            if surface.pixels[row * stride + column] == want {
                found += 1;
            }
        }
    }
    found
}

fn paint(screen: &Screen) -> (Surface, usize) {
    let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
    let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
    let mut surface = Surface::new(stride, height);
    surface.clear(tone::PAPER);
    render(screen, &mut surface, None);
    (surface, stride)
}

fn a_status() -> Status {
    Status::standard("09:41", Some(Percent::new(64)), false, Signal::Fair, false)
}

#[test]
fn the_band_sits_above_the_bar_rather_than_over_it() {
    // Drawn without moving anything, the band would cover the title and
    // the way back -- the one control that must always work.
    let screen = Screen::new(1, vec![]).with_top_bar(TopBar::new(NodeId(0), "Feeds"));
    let bare = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    let banded = screen.layout_with(
        &CLARA_BW_METRICS,
        &Chrome::with_back(true).with_status(a_status()),
    );
    let find = |layout: &Layout, kind: LayoutKind| {
        layout
            .nodes
            .iter()
            .find(|node| node.kind == kind)
            .map(|node| node.rect)
    };
    let band = find(&banded, LayoutKind::StatusBand).expect("the band was laid out");
    assert_eq!(band.y, 0, "the band was not at the top of the panel");
    assert_eq!(band.height, CLARA_BW_METRICS.status_band_height());
    let bar = find(&banded, LayoutKind::TopBar).expect("the bar was laid out");
    assert_eq!(
        bar.y, band.height,
        "the top bar did not move down by the height of the band"
    );
    let back = find(&banded, LayoutKind::Back).expect("the way back was laid out");
    assert!(
        back.y >= band.y + band.height,
        "the band was drawn over the way back"
    );
    let before = find(&bare, LayoutKind::TopBarTitle).expect("a title");
    let after = find(&banded, LayoutKind::TopBarTitle).expect("a title");
    assert_eq!(
        after.y - before.y,
        band.height,
        "the title did not move with its bar"
    );
}

#[test]
fn content_starts_below_the_band_rather_than_under_the_title() {
    // The fault this rules out reached real hardware: the top bar moved
    // down by the band and the content did not, so the launcher's first
    // row of tiles was laid out underneath its own title. It could not be
    // seen in simulation, because the simulator drew no band and a band of
    // zero height makes the wrong arithmetic right.
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "A paragraph the reader is meant to be able to read.".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_top_bar(TopBar::new(NodeId(0), "Feeds"));
    let banded = screen.layout_with(
        &CLARA_BW_METRICS,
        &Chrome::with_back(true).with_status(a_status()),
    );
    let find = |layout: &Layout, kind: LayoutKind| {
        layout
            .nodes
            .iter()
            .find(|node| node.kind == kind)
            .map(|node| node.rect)
    };
    let bar = find(&banded, LayoutKind::TopBar).expect("the bar was laid out");
    let rule = find(&banded, LayoutKind::Divider).expect("the rule under the bar");
    let text = find(&banded, LayoutKind::Text).expect("the paragraph was laid out");
    assert!(
        text.y >= bar.y + bar.height,
        "content at y={} was drawn inside a bar ending at y={}",
        text.y,
        bar.y + bar.height
    );
    assert!(
        text.y >= rule.y + rule.height,
        "content was drawn over the rule under the bar"
    );
    // And the whole content area, not just the first node in it: the page
    // turn zones and every measurement of what fits are taken from this.
    assert!(
        banded.content.y >= bar.y + bar.height,
        "the content area began inside the top bar"
    );
}

#[test]
fn a_book_is_not_drawn_with_a_clock_on_it() {
    // Withheld by the runtime rather than by the layout, so this checks
    // the layout honours an absent status rather than inventing one.
    let screen = Screen::new(1, vec![])
        .with_reading(true)
        .with_top_bar(TopBar::new(NodeId(0), "Moby-Dick"));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    assert!(
        !layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::StatusBand),
        "a band was drawn on a screen whose chrome asked for none"
    );
}

#[test]
fn the_battery_is_drawn_at_the_level_it_was_read_at() {
    // The icon in the glyph set is a symbol: it means "battery" and says
    // nothing about charge. A band that showed it would tell the reader
    // their device has a battery, which they know.
    let mark = |percent: u8| {
        let shapes = vector::battery(Percent::new(percent), false);
        let coverage = vector::render(&shapes, 64);
        (0..64)
            .flat_map(|row| (0..64).map(move |column| (column, row)))
            .filter(|&(column, row)| coverage.at(column, row) > 128)
            .count()
    };
    let (empty, half, full) = (mark(0), mark(50), mark(100));
    assert!(
        half > empty && full > half,
        "the battery did not get darker as it filled: {empty}, {half}, {full}"
    );
    // A charging battery says so rather than showing a level, because the
    // level of a battery on a charger is the least interesting thing about
    // it and this panel cannot afford an animated fill.
    let charging = vector::battery(Percent::new(0), true);
    let coverage = vector::render(&charging, 64);
    assert!(
        (0..64)
            .flat_map(|row| (0..64).map(move |column| (column, row)))
            .any(|(column, row)| coverage.at(column, row) > 128),
        "a charging battery at zero percent was drawn as an empty shell"
    );
}

#[test]
fn no_radio_is_a_different_mark_from_a_weak_one() {
    // Zero arcs and one arc differ by one small stroke, which at eight
    // pixels tall is not a difference a reader can act on -- and the
    // difference between "not connected" and "barely connected" is the
    // whole reason to look.
    let ink = |strength: Signal| {
        let coverage = vector::render(&vector::wifi(strength), 64);
        (0..64)
            .flat_map(|row| (0..64).map(move |column| (column, row)))
            .filter(|&(column, row)| coverage.at(column, row) > 128)
            .count()
    };
    let (weak, fair, strong) = (ink(Signal::Weak), ink(Signal::Fair), ink(Signal::Strong));
    assert!(
        weak < fair && fair < strong,
        "arcs did not accumulate with strength: {weak}, {fair}, {strong}"
    );
    // The off mark is struck through, so it carries ink where no arc does:
    // along the diagonal. Comparing quantity alone would pass for a mark
    // that was merely a fainter weak one.
    let off = vector::render(&vector::wifi(Signal::Off), 64);
    let weak_mark = vector::render(&vector::wifi(Signal::Weak), 64);
    let diagonal = (8..56)
        .filter(|&step| off.at(step, step) > 128 && weak_mark.at(step, step) == 0)
        .count();
    assert!(
        diagonal > 8,
        "the no-radio mark is not struck through, so it reads as a weak signal"
    );
}

#[test]
fn the_strength_thresholds_are_the_ones_the_stock_reader_uses() {
    assert_eq!(Signal::from_dbm(-45), Signal::Strong);
    assert_eq!(Signal::from_dbm(-60), Signal::Strong);
    assert_eq!(Signal::from_dbm(-61), Signal::Fair);
    assert_eq!(Signal::from_dbm(-70), Signal::Fair);
    assert_eq!(Signal::from_dbm(-71), Signal::Weak);
    assert_eq!(Signal::from_dbm(-95), Signal::Weak);
}

fn band_marks(status: Status, metrics: &DisplayMetrics) -> Vec<(StatusMark, Rect)> {
    let screen = Screen::new(1, vec![]).with_top_bar(TopBar::new(NodeId(0), "Cobalt"));
    let chrome = Chrome::with_back(false).with_status(status);
    screen
        .layout_with(metrics, &chrome)
        .nodes
        .into_iter()
        .filter_map(|node| match node.kind {
            LayoutKind::StatusMark(mark) => Some((mark, node.rect)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_band_is_drawn_in_ink_and_paper_only() {
    // Anything else in the band selects a greyscale refresh each time the
    // clock ticks or the charge moves, and draws soft figures beside sharp
    // marks.
    let screen = Screen::new(1, vec![]).with_top_bar(TopBar::new(NodeId(0), "Cobalt"));
    let chrome = Chrome::with_back(false).with_status(Status::standard(
        "09:41",
        Some(Percent::new(64)),
        true,
        Signal::Fair,
        true,
    ));
    let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
    let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
    let mut surface = Surface::new(stride, height);
    surface.clear(tone::PAPER);
    render_with(&screen, &CLARA_BW_METRICS, &chrome, &mut surface, None);
    let band = usize::try_from(CLARA_BW_METRICS.status_band_height()).expect("a height");
    let grey = surface.pixels[..band * stride]
        .iter()
        .filter(|&&value| value != tone::PAPER && value != tone::INK)
        .count();
    assert_eq!(grey, 0, "{grey} grey pixels in the status band");
}

#[test]
fn marks_are_placed_from_the_trailing_edge_in_the_order_given() {
    let marks = band_marks(
        Status::standard("09:41", Some(Percent::new(80)), false, Signal::Fair, true),
        &CLARA_BW_METRICS,
    );
    let kinds: Vec<_> = marks.iter().map(|(mark, _)| *mark).collect();
    assert_eq!(
        kinds,
        vec![
            StatusMark::Battery {
                level: Some(Percent::new(80)),
                charging: false
            },
            StatusMark::Signal(Signal::Fair),
            StatusMark::Bluetooth,
        ]
    );
    for pair in marks.windows(2) {
        assert!(
            pair[1].1.x + pair[1].1.width <= pair[0].1.x,
            "{:?} overlaps {:?}",
            pair[1].0,
            pair[0].0
        );
    }
}

#[test]
fn a_band_too_narrow_for_every_mark_drops_the_innermost_first() {
    // Ten Bluetooth marks cannot fit beside the clock. The battery and the
    // radio are listed first and must survive; none may reach the clock.
    let mut status = Status::standard("09:41", Some(Percent::new(80)), false, Signal::Fair, false);
    status
        .marks
        .extend(std::iter::repeat_n(StatusMark::Bluetooth, 30));
    let marks = band_marks(status, &CLARA_BW_METRICS);
    assert!(
        marks.len() < 32,
        "every mark was placed, so nothing was tested"
    );
    assert!(matches!(marks[0].0, StatusMark::Battery { .. }));
    assert_eq!(marks[1].0, StatusMark::Signal(Signal::Fair));
    let clock_end =
        CLARA_BW_METRICS.screen_margin() + figures_width("09:41", FontSize::Caption, Face::Text);
    let innermost = marks.last().expect("some marks").1;
    assert!(innermost.x > clock_end, "a mark was drawn over the clock");
}

#[test]
fn the_radio_does_not_move_as_the_battery_number_changes_width() {
    let signal_x = |level| {
        band_marks(
            Status::standard(
                "09:41",
                Some(Percent::new(level)),
                false,
                Signal::Fair,
                false,
            ),
            &CLARA_BW_METRICS,
        )
        .into_iter()
        .find(|(mark, _)| matches!(mark, StatusMark::Signal(_)))
        .expect("a radio mark")
        .1
        .x
    };
    assert_eq!(signal_x(5), signal_x(100));
}

#[test]
fn an_unreadable_battery_is_drawn_as_nothing_rather_than_as_empty() {
    // An empty battery and an unreadable one look identical and mean the
    // opposite things, so the honest drawing of "unknown" is no drawing.
    let screen = Screen::new(1, vec![]).with_top_bar(TopBar::new(NodeId(0), "Cobalt"));
    let status = Status::standard("09:41", None, false, Signal::Fair, false);
    let chrome = Chrome::with_back(false).with_status(status);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &chrome);
    let rect = layout
        .nodes
        .iter()
        .find(|node| {
            matches!(
                node.kind,
                LayoutKind::StatusMark(StatusMark::Battery { .. })
            )
        })
        .expect("the slot is still reserved so the marks do not shuffle")
        .rect;
    let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
    let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
    let mut surface = Surface::new(stride, height);
    surface.clear(tone::PAPER);
    render_with(&screen, &CLARA_BW_METRICS, &chrome, &mut surface, None);
    let drawn = (rect.y..rect.y + rect.height)
        .flat_map(|row| (rect.x..rect.x + rect.width).map(move |column| (column, row)))
        .filter(|&(column, row)| {
            let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
                return false;
            };
            surface.pixels[row * stride + column] != tone::PAPER
        })
        .count();
    assert_eq!(drawn, 0, "an unreadable battery was drawn as an empty one");
}

#[test]
fn an_ordinary_control_is_outlined_and_only_the_primary_one_is_filled() {
    // Every enabled button used to be a filled black slab, so a screen of
    // three choices was three identical black bars with nothing to aim at,
    // and the panel had the most expensive mark it can draw repeated three
    // times. Only the action the screen exists for is filled now.
    let screen = Screen::new(
        1,
        vec![
            Node::Button {
                id: NodeId(1),
                action: ActionId(1),
                label: "Read".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Primary,
            },
            Node::Button {
                id: NodeId(2),
                action: ActionId(2),
                label: "Details".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            },
        ],
    );
    let layout = screen.layout();
    let rect = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("the control was laid out")
            .rect
    };
    let (surface, stride) = paint(&screen);
    let filled = tone_count(&surface, rect(1), tone::INK, stride);
    let outlined = tone_count(&surface, rect(2), tone::INK, stride);
    let area = usize::try_from(rect(1).width * rect(1).height).expect("a real control");
    assert!(
        filled * 10 > area * 8,
        "the primary control was not filled: {filled} of {area}"
    );
    assert!(
        outlined * 4 < area,
        "the ordinary control was filled too: {outlined} of {area}"
    );
    assert!(
        outlined > 0,
        "the ordinary control left no mark at all, so it cannot be found"
    );
}

#[test]
fn an_ordinary_control_still_reads_as_available_next_to_a_disabled_one() {
    // Both are outlined, so what separates them has to be the weight of
    // the rule and the tone of the label rather than the shape.
    let screen = Screen::new(
        1,
        vec![
            Node::Button {
                id: NodeId(1),
                action: ActionId(1),
                label: "Subscribe".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            },
            Node::Button {
                id: NodeId(2),
                action: ActionId(2),
                label: "Subscribe".to_owned(),
                state: ControlState::Disabled,
                emphasis: Emphasis::Normal,
            },
        ],
    );
    let layout = screen.layout();
    let rect = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("laid out")
            .rect
    };
    let (surface, stride) = paint(&screen);
    assert!(
        tone_count(&surface, rect(1), tone::INK, stride)
            > tone_count(&surface, rect(2), tone::INK, stride),
        "an available control was no darker than one that cannot be used"
    );
    assert_eq!(
        tone_count(&surface, rect(1), tone::MUTED, stride),
        0,
        "an available control was drawn in the tone reserved for refusal"
    );
}

#[test]
fn an_attention_banner_marks_its_edge_rather_than_blacking_out_the_panel() {
    // It used to be reversed out of solid black across the full width,
    // which shouted louder than whatever it was warning about and left the
    // panel a slab to clear before anything near it could be redrawn.
    let screen = Screen::new(
        1,
        vec![Node::Banner {
            id: NodeId(1),
            level: BannerLevel::Attention,
            text: "The network went away.".to_owned(),
        }],
    );
    let rect = screen
        .layout()
        .nodes
        .into_iter()
        .find(|node| matches!(node.kind, LayoutKind::Banner(_)))
        .expect("the banner was laid out")
        .rect;
    let (surface, stride) = paint(&screen);
    let ink = tone_count(&surface, rect, tone::INK, stride);
    let area = usize::try_from(rect.width * rect.height).expect("a real banner");
    assert!(
        ink * 4 < area,
        "the banner is still mostly black: {ink} of {area}"
    );
    // The leading edge is the mark that distinguishes it, so it must be
    // solid ink for the full height of the banner.
    let bar = CLARA_BW_METRICS.rule_thickness() * 3;
    let edge = Rect { width: bar, ..rect };
    assert_eq!(
        tone_count(&surface, edge, tone::INK, stride),
        usize::try_from(bar * rect.height).expect("a real edge"),
        "the leading edge was not drawn solid"
    );
}

#[test]
fn metadata_is_lighter_and_smaller_than_what_it_describes() {
    // A heading followed by an undifferentiated column is what made every
    // screen read as one block of prose. A caption is set like a caption.
    let screen = Screen::new(
        1,
        vec![
            Node::Text {
                id: NodeId(1),
                text: "Twenty Thousand Leagues Under the Sea".to_owned(),
                links: Vec::new(),
            },
            Node::Secondary {
                id: NodeId(2),
                text: "Jules Verne, 1870".to_owned(),
            },
        ],
    );
    let layout = screen.layout();
    let node = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("laid out")
            .clone()
    };
    assert_eq!(node(2).kind, LayoutKind::Secondary);
    assert!(
        node(2).rect.height < node(1).rect.height,
        "the caption took as much room as the line it describes"
    );
    let (surface, stride) = paint(&screen);
    assert_eq!(
        tone_count(&surface, node(2).rect, tone::INK, stride),
        0,
        "metadata was drawn in full-strength ink"
    );
    assert!(
        tone_count(&surface, node(2).rect, tone::MUTED, stride) > 0,
        "metadata was not drawn at all"
    );
}

#[test]
fn depth_is_counted_in_rules_rather_than_left_to_the_indent() {
    // Two millimetres of indent per level is invisible on the panel: on a
    // photograph of a real thread a reply and a reply-to-a-reply looked
    // the same. So every ancestor gets a rule, and the count is readable
    // even when the measure is not.
    let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
    let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
    for depth in 0..=3u8 {
        let screen = Screen::new(
            1,
            vec![Node::Quote {
                id: NodeId(1),
                depth,
                role: QuoteRole::Body,
                fold: None,
                text: "A reply, which answers the one above it.".to_owned(),
            }],
        );
        let quote = screen
            .layout()
            .nodes
            .into_iter()
            .find(|node| matches!(node.kind, LayoutKind::Quote(..)))
            .expect("the reply was laid out");
        let mut surface = Surface::new(stride, height);
        surface.clear(tone::PAPER);
        render(&screen, &mut surface, None);

        // Count columns of rule ink to the left of the text, one row down
        // into the paragraph so no ascender can be mistaken for a rule.
        let row = usize::try_from(quote.rect.y + quote.rect.height / 2).expect("inside");
        let left = usize::try_from(quote.rect.x).expect("inside the panel");
        let mut columns = 0;
        let mut inside_rule = false;
        for column in 0..left {
            let ink = surface.pixels[row * stride + column] == tone::RULE;
            if ink && !inside_rule {
                columns += 1;
            }
            inside_rule = ink;
        }
        assert_eq!(
            columns,
            usize::from(depth),
            "a reply at depth {depth} was marked with {columns} rules"
        );
    }
}

#[test]
fn a_byline_is_set_apart_from_what_it_introduces() {
    // The author line was emitted as an ordinary paragraph: same size,
    // same ink, so a thread read as one undifferentiated column and a page
    // could open on a comment with no visible author at all.
    let body = Screen::new(
        1,
        vec![Node::Quote {
            id: NodeId(1),
            depth: 1,
            role: QuoteRole::Body,
            fold: None,
            text: "patio11 2 hours ago".to_owned(),
        }],
    );
    let byline = Screen::new(
        1,
        vec![Node::Quote {
            id: NodeId(1),
            depth: 1,
            role: QuoteRole::Byline,
            fold: None,
            text: "patio11 2 hours ago".to_owned(),
        }],
    );
    assert_eq!(QuoteRole::Byline.size(), FontSize::Caption);
    assert_eq!(QuoteRole::Byline.tone(), tone::MUTED);
    assert!(
        QuoteRole::Byline.size().tenth_mm() < QuoteRole::Body.size().tenth_mm(),
        "the byline was not smaller than the comment it introduces"
    );
    let measure = |screen: &Screen| {
        screen
            .layout()
            .nodes
            .into_iter()
            .find(|node| matches!(node.kind, LayoutKind::Quote(..)))
            .expect("the line was laid out")
            .rect
            .height
    };
    assert!(
        measure(&byline) < measure(&body),
        "the byline took as much room as a paragraph"
    );
}

#[test]
fn a_page_that_opens_mid_comment_says_whose_it_is() {
    // Turning the page used to lose the author: the byline was on the page
    // before, and what followed was an unattributed wall.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let long = book(DESCRIPTION, 40);
    let mut source = vec![(1u8, QuoteRole::Byline, "patio11 2 hours ago".to_owned())];
    source.extend(
        long.split("\n\n")
            .map(|paragraph| (1, QuoteRole::Body, paragraph.to_owned())),
    );
    let borrowed = source
        .iter()
        .map(|(depth, role, text)| (*depth, *role, text.as_str()))
        .collect::<Vec<_>>();
    let pages = paginate_quoted(&borrowed, &CLARA_BW_METRICS, area);
    assert!(pages.len() > 1, "one page cannot break mid-comment");
    for (index, page) in pages.iter().enumerate().skip(1) {
        let (_, role, text) = page.first().expect("a page is never empty");
        assert_eq!(
            *role,
            QuoteRole::Byline,
            "page {index} opened on a comment with no author"
        );
        assert!(
            text.starts_with("patio11"),
            "page {index} named somebody else: {text}"
        );
    }
}

#[test]
fn the_repeated_byline_is_paid_for_out_of_the_page_it_opens() {
    // A continuation line that is drawn but not measured is how the last
    // paragraph of a page ends up below the panel.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let long = book(DESCRIPTION, 40);
    let mut source = vec![(1u8, QuoteRole::Byline, "patio11 2 hours ago".to_owned())];
    source.extend(
        long.split("\n\n")
            .map(|paragraph| (1, QuoteRole::Body, paragraph.to_owned())),
    );
    let borrowed = source
        .iter()
        .map(|(depth, role, text)| (*depth, *role, text.as_str()))
        .collect::<Vec<_>>();
    let floor = CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height();
    for (index, page) in paginate_quoted(&borrowed, &CLARA_BW_METRICS, area)
        .iter()
        .enumerate()
    {
        let (shown, bottom) = drawn_quoted(page, &CLARA_BW_METRICS);
        assert_eq!(shown, page.len(), "page {index} lost a paragraph");
        assert!(
            bottom <= floor,
            "page {index} ran {} pixels under the page controls",
            bottom - floor
        );
    }
}

#[test]
fn a_page_of_dialogue_holds_less_than_a_page_of_description() {
    // This is the whole reason pagination measures rather than counting
    // characters: short paragraphs spend most of the page on the gaps
    // between them, so one budget cannot serve both.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let description: usize = paginate(&book(DESCRIPTION, 12), area)[0]
        .iter()
        .map(|paragraph| paragraph.chars().count())
        .sum();
    let dialogue: usize = paginate(&book(DIALOGUE, 12), area)[0]
        .iter()
        .map(|paragraph| paragraph.chars().count())
        .sum();
    assert!(
        dialogue < description,
        "dialogue {dialogue} was not less than description {description}"
    );
}

#[test]
fn no_words_are_lost_between_pages() {
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let source = book(DESCRIPTION, 9);
    let pages = paginate(&source, area);
    assert!(pages.len() > 1, "the sample fitted on one page");
    let paginated = pages
        .iter()
        .flat_map(|page| page.iter())
        .flat_map(|paragraph| paragraph.split_whitespace())
        .collect::<Vec<_>>();
    assert_eq!(paginated, source.split_whitespace().collect::<Vec<_>>());
}

#[test]
fn a_paragraph_longer_than_a_page_is_split_rather_than_dropped() {
    // Gutenberg's front matter is sometimes one unbroken block, and a
    // reader that dropped it would open books at chapter two.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let monster = "word ".repeat(4000);
    let pages = paginate(&monster, area);
    assert!(pages.len() > 1);
    for page in &pages {
        let (shown, bottom) = drawn(page, &CLARA_BW_METRICS);
        assert_eq!(shown, page.len());
        assert!(bottom <= CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height());
    }
}

#[test]
fn a_source_line_break_does_not_become_a_short_line_on_the_panel() {
    // Gutenberg hard wraps its plain text at about seventy columns. Taking
    // those as real breaks would give a narrow ragged column down the
    // middle of a panel that is wider than seventy characters.
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let pages = paginate("one two three\nfour five six\nseven eight", area);
    assert_eq!(pages, vec![vec!["one two three four five six seven eight"]]);
}

#[test]
fn an_area_too_small_for_a_line_produces_no_pages_rather_than_panicking() {
    let pages = paginate(
        DESCRIPTION,
        ProseArea {
            width: 400,
            height: 2,
            gap: 4,
            face: Face::Text,
        },
    );
    assert!(pages.is_empty());
}

/// The hazard that cost this project a working button once already.
///
/// A screen whose text above a control grows by one wrapped line pushes
/// that control down. On a panel that takes a moment to refresh, the
/// control moves out from under the finger that just tapped it and the
/// next tap lands on nothing, which looks like intermittent hardware.
///
/// This pins both halves: that the layout engine really does move it, so
/// nobody has to take the hazard on trust, and that keeping the varying
/// text below the control fixes it. Applications are written to the second
/// shape; this is why.
#[test]
fn text_that_wraps_above_a_control_moves_that_control() {
    let action = ActionId(7);
    let button = |before: &str, after: &str| -> Rect {
        let mut nodes = Vec::new();
        if !before.is_empty() {
            nodes.push(Node::Text {
                id: NodeId(1),
                text: before.to_string(),
                links: Vec::new(),
            });
        }
        nodes.push(Node::Button {
            id: NodeId(2),
            action,
            label: "Do it".to_string(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        });
        if !after.is_empty() {
            nodes.push(Node::Text {
                id: NodeId(3),
                text: after.to_string(),
                links: Vec::new(),
            });
        }
        Screen::new(1, nodes)
            .layout_for(&CLARA_BW_METRICS)
            .rect_of_action(action)
            .expect("the button is always drawn")
    };
    let short = "Ready.";
    let long = concat!(
        "Ready, and then a great deal more text than that, enough of it ",
        "to run past the end of one line and onto a second one.",
    );

    assert_ne!(
        button(short, ""),
        button(long, ""),
        "a longer line above the button has to move it, or this test proves nothing"
    );
    assert_eq!(
        button("", short),
        button("", long),
        "text below the button must not move it"
    );
}

#[test]
fn a_picture_is_never_enlarged_to_fill_its_space() {
    // Upscaling a thumbnail is how a sharp cover becomes a soft one, and
    // softness is the one thing a sixteen-grey panel cannot hide.
    assert_eq!(fit_within((190, 300), 800, 800), (190, 300));
}

#[test]
fn a_picture_too_wide_and_too_tall_keeps_its_proportions() {
    let (width, height) = fit_within((1000, 500), 400, 400);
    assert_eq!((width, height), (400, 200));
    let (width, height) = fit_within((500, 1000), 400, 400);
    assert_eq!((width, height), (200, 400));
}

#[test]
fn a_portrait_tile_is_taller_than_a_square_one() {
    let tiles = || vec![Tile::new(ActionId(7), "Moby Dick", Glyph::Book)];
    let square = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            tiles: tiles(),
            shape: TileShape::Square,
        }],
    )
    .layout();
    let portrait = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            tiles: tiles(),
            shape: TileShape::Portrait,
        }],
    )
    .layout();
    let height = |layout: &Layout| {
        layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Tile(..)))
            .expect("a tile")
            .rect
            .height
    };
    assert!(
        height(&portrait) > height(&square),
        "a shelf of covers has to be book shaped, not stamp shaped"
    );
}

/// A cover on a shelf keeps the pale edge that tells it from the page.
///
/// The outline lives on the layout's kind rather than on the drawing, so
/// it is lost by describing a picture as the unframed sort. A book cover
/// is very often white at its margins, and without the edge it bleeds into
/// the shelf behind it.
#[test]
fn a_cover_on_a_shelf_keeps_its_edge() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            tiles: vec![Tile::new(ActionId(8), "Arrived", Glyph::Book)
                .with_picture(TilePicture::new(PictureHandle(3), 190, 300))],
            shape: TileShape::Portrait,
        }],
    )
    .layout();
    assert!(
        screen
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::FramedPicture(PictureHandle(3))),
        "a cover was drawn without the edge that separates it from the shelf"
    );
}

#[test]
fn a_tile_without_its_picture_yet_still_shows_its_glyph() {
    // Covers arrive one network request at a time, so most of a shelf's
    // life is spent with some of them missing. That has to be a usable
    // screen rather than a broken one.
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            tiles: vec![
                Tile::new(ActionId(7), "Waiting", Glyph::Book),
                Tile::new(ActionId(8), "Arrived", Glyph::Book).with_picture(TilePicture::new(
                    PictureHandle(3),
                    190,
                    300,
                )),
            ],
            shape: TileShape::Portrait,
        }],
    )
    .layout();
    assert_eq!(
        screen
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::TileGlyph(_)))
            .count(),
        1
    );
    assert_eq!(
        screen
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::Picture(_) | LayoutKind::FramedPicture(_)
                )
            })
            .count(),
        1
    );
    assert_eq!(
        screen
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::TileLabel)
            .count(),
        2,
        "both tiles keep their label whatever is above it"
    );
}

#[test]
fn the_cache_refuses_a_picture_whose_size_does_not_match_its_bytes() {
    let mut cache = PictureCache::default();
    assert!(!cache.put(PictureHandle(1), 10, 10, vec![0; 99]));
    assert!(cache.get(PictureHandle(1)).is_none());
    assert!(cache.put(PictureHandle(1), 10, 10, vec![0; 100]));
    assert_eq!(cache.get(PictureHandle(1)).map(|p| p.width), Some(10));
}

#[test]
fn the_cache_evicts_what_was_drawn_longest_ago() {
    let mut cache = PictureCache::new(200);
    assert!(cache.put(PictureHandle(1), 10, 10, vec![1; 100]));
    assert!(cache.put(PictureHandle(2), 10, 10, vec![2; 100]));
    // Drawing the first one makes the second the older of the two.
    assert!(cache.get(PictureHandle(1)).is_some());
    assert!(cache.put(PictureHandle(3), 10, 10, vec![3; 100]));
    assert!(cache.get(PictureHandle(1)).is_some(), "still on screen");
    assert!(
        cache.get(PictureHandle(2)).is_none(),
        "least recently drawn"
    );
    assert!(cache.get(PictureHandle(3)).is_some());
    assert_eq!(cache.bytes_held(), 200);
}

#[test]
fn cache_evictions_are_reported_to_the_runtime() {
    let mut cache = PictureCache::new(150);
    assert_eq!(
        cache.put_report(PictureHandle(1), 10, 10, vec![1; 100]),
        Some(Vec::new())
    );
    assert_eq!(
        cache.put_report(PictureHandle(2), 10, 10, vec![2; 100]),
        Some(vec![PictureHandle(1)])
    );
}

#[test]
fn chunked_picture_becomes_live_only_after_a_complete_commit() {
    let mut cache = PictureCache::new(300);
    assert!(cache.begin_upload(PictureHandle(7), 10, 10));
    assert!(cache.upload_chunk(PictureHandle(7), 0, &[3; 40]));
    assert!(
        cache.get(PictureHandle(7)).is_none(),
        "not partially visible"
    );
    assert!(cache.upload_chunk(PictureHandle(7), 40, &[3; 60]));
    assert_eq!(cache.commit_upload(PictureHandle(7)), Some(Vec::new()));
    assert_eq!(cache.get(PictureHandle(7)).map(|p| p.grey[0]), Some(3));
}

#[test]
fn an_out_of_order_chunk_cancels_the_upload() {
    let mut cache = PictureCache::new(300);
    assert!(cache.begin_upload(PictureHandle(7), 10, 10));
    assert!(!cache.upload_chunk(PictureHandle(7), 1, &[3; 40]));
    assert_eq!(cache.commit_upload(PictureHandle(7)), None);
}

#[test]
fn replacing_a_picture_does_not_double_count_it() {
    let mut cache = PictureCache::new(300);
    assert!(cache.put(PictureHandle(1), 10, 10, vec![0; 100]));
    assert!(cache.put(PictureHandle(1), 10, 10, vec![9; 100]));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes_held(), 100);
    assert_eq!(cache.get(PictureHandle(1)).map(|p| p.grey[0]), Some(9));
}

#[test]
fn a_picture_is_drawn_where_it_was_placed_and_nowhere_else() {
    let mut cache = PictureCache::default();
    assert!(cache.put(PictureHandle(1), 2, 2, vec![0, 0, 0, 0]));
    let mut surface = Surface::new(8, 8);
    surface.clear(tone::PAPER);
    let rect = Rect {
        x: 2,
        y: 3,
        width: 2,
        height: 2,
    };
    let clip = Rect {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    };
    draw_picture(
        &mut surface,
        rect,
        cache.get(PictureHandle(1)).expect("held"),
        clip,
    );
    for y in 0..8 {
        for x in 0..8 {
            let inside = (2..4).contains(&x) && (3..5).contains(&y);
            let pixel = surface.pixels[y * 8 + x];
            assert_eq!(
                pixel == 0,
                inside,
                "pixel ({x},{y}) should {} be ink",
                if inside { "" } else { "not" }
            );
        }
    }
}

#[test]
fn a_colour_picture_is_held_with_its_grey_and_drawn_in_colour() {
    let mut cache = PictureCache::new(64);
    let rgb = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 90, 90, 90];
    assert_eq!(
        cache.put_report_with(PictureHandle(1), 2, 2, PictureFormat::Rgb, rgb.clone()),
        Some(Vec::new())
    );
    // Charged for both planes: twelve colour bytes and four grey ones.
    assert_eq!(cache.bytes_held(), 16);
    let held = cache.get(PictureHandle(1)).expect("held");
    assert_eq!(held.colour, Some(rgb.as_slice()));
    assert_eq!(
        held.grey,
        &[luma([255, 0, 0]), luma([0, 255, 0]), luma([0, 0, 255]), 90]
    );
    // The declared size is checked against the format's byte count.
    assert!(cache
        .put_report_with(PictureHandle(2), 2, 2, PictureFormat::Rgb, vec![0; 4])
        .is_none());
    assert!(cache
        .put_report_with(PictureHandle(2), 2, 2, PictureFormat::Grey, vec![0; 12])
        .is_none());

    let mut surface = Surface::new(4, 4);
    draw_picture(
        &mut surface,
        Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        },
        cache.get(PictureHandle(1)).expect("held"),
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        },
    );
    assert_eq!(surface.rgb_at(5), Some([255, 0, 0]));
    assert_eq!(surface.rgb_at(6), Some([0, 255, 0]));
    assert_eq!(surface.rgb_at(9), Some([0, 0, 255]));
    assert_eq!(surface.rgb_at(10), Some([90, 90, 90]));
    assert_eq!(surface.pixels[5], luma([255, 0, 0]));
    assert_eq!(surface.rgb_at(0), Some([tone::PAPER; 3]));

    // Shrunk, the colours average per channel.
    let mut small = Surface::new(1, 1);
    draw_picture(
        &mut small,
        Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        cache.get(PictureHandle(1)).expect("held"),
        Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
    );
    assert_eq!(
        small.rgb_at(0),
        Some([86, 86, 86]),
        "each channel is the mean of 255, 0, 0 and 90"
    );
}

#[test]
fn a_colour_upload_is_committed_in_its_format() {
    let mut cache = PictureCache::new(64);
    assert!(cache.begin_upload_with(PictureHandle(3), 2, 1, PictureFormat::Rgb));
    assert!(cache.upload_chunk(PictureHandle(3), 0, &[255, 0, 0]));
    assert!(cache.upload_chunk(PictureHandle(3), 3, &[0, 0, 255]));
    assert_eq!(cache.commit_upload(PictureHandle(3)), Some(Vec::new()));
    let held = cache.get(PictureHandle(3)).expect("held");
    assert_eq!(held.colour, Some(&[255, 0, 0, 0, 0, 255][..]));
    assert_eq!(held.grey.len(), 2);
    // A colour picture whose two planes overflow the budget is refused at
    // commit even though its bytes alone fitted in flight.
    let mut tight = PictureCache::new(12);
    assert!(tight.begin_upload_with(PictureHandle(4), 2, 2, PictureFormat::Rgb));
    assert!(tight.upload_chunk(PictureHandle(4), 0, &[0; 12]));
    assert_eq!(tight.commit_upload(PictureHandle(4)), None);
}

#[test]
fn shrinking_a_picture_averages_rather_than_drops_pixels() {
    // Half the source is black and half white. Sampling would give one or
    // the other; averaging gives the grey that is actually there.
    let mut cache = PictureCache::default();
    assert!(cache.put(PictureHandle(1), 2, 2, vec![0, 255, 0, 255]));
    let mut surface = Surface::new(4, 4);
    surface.clear(tone::PAPER);
    let rect = Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    };
    draw_picture(
        &mut surface,
        rect,
        cache.get(PictureHandle(1)).expect("held"),
        Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        },
    );
    assert_eq!(surface.pixels[0], 127);
}

fn a_byline(fold: Option<Fold>) -> Screen {
    Screen::new(
        1,
        vec![
            Node::Quote {
                id: NodeId(1),
                depth: 0,
                role: QuoteRole::Byline,
                text: "someone 3 hours ago".to_owned(),
                fold,
            },
            Node::Quote {
                id: NodeId(2),
                depth: 0,
                role: QuoteRole::Body,
                text: "What they said.".to_owned(),
                fold: None,
            },
        ],
    )
}

#[test]
fn a_byline_is_set_on_a_tint_and_the_words_underneath_it_are_not() {
    // Muted caption text alone was not enough to tell a name apart from a
    // sentence, so the byline gets a band. The band has to stop at the
    // byline: a comment drawn entirely on a tint is a card, and a page of
    // cards is a page with no white space in it.
    let screen = a_byline(None);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let (surface, stride) = paint(&screen);
    let byline = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Byline)))
        .expect("a byline");
    let body = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Body)))
        .expect("a body");
    assert!(
        tone_count(&surface, byline.rect, tone::SURFACE, stride) > 0,
        "the byline was drawn on bare paper, so it still reads as a sentence"
    );
    assert_eq!(
        tone_count(&surface, body.rect, tone::SURFACE, stride),
        0,
        "the tint ran on past the byline and under the comment"
    );
}

#[test]
fn a_fold_does_not_move_the_words_when_it_opens_and_shuts() {
    // The mark and its count are drawn in room taken out of the byline up
    // front. Sizing that room to the state would re-wrap the line under
    // the finger that just tapped it, which on a panel that takes half a
    // second to repaint reads as the text running away.
    let shut = a_byline(Some(Fold {
        action: ActionId(7),
        collapsed: true,
        hidden: 12,
    }));
    let open = a_byline(Some(Fold {
        action: ActionId(7),
        collapsed: false,
        hidden: 12,
    }));
    let lines = |screen: &Screen| {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Byline)))
            .expect("a byline")
            .text_lines
            .clone()
    };
    assert_eq!(
        lines(&shut),
        lines(&open),
        "the byline was wrapped differently open and shut"
    );

    // The rectangle is the whole column either way, because that is the
    // band the byline is drawn on. What the mark takes is taken out of the
    // words, so the claim has to be made about the words: a byline long
    // enough to fill the column has to break earlier once it is foldable,
    // or the mark is drawn over the end of it.
    let long = "somebodywithaverylongname and then a great deal more of a byline than \
                    anybody would ever write, long enough to fill the column twice over";
    let wrapped = |fold| {
        let mut screen = a_byline(fold);
        let Node::Quote { text, .. } = &mut screen.nodes[0] else {
            unreachable!("the first node is the byline")
        };
        *text = long.to_owned();
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Byline)))
            .expect("a byline")
            .text_lines[0]
            .clone()
    };
    let foldable = wrapped(Some(Fold {
        action: ActionId(7),
        collapsed: false,
        hidden: 3,
    }));
    assert!(
        foldable.len() < wrapped(None).len(),
        "a foldable byline used the full column, so the mark is drawn over its words"
    );
}

#[test]
fn the_whole_byline_is_the_handle_not_just_the_mark() {
    // A plus sign is about three millimetres across. Asking a finger to
    // find it is asking for a miss, and a miss on a thread turns the page.
    let screen = a_byline(Some(Fold {
        action: ActionId(7),
        collapsed: false,
        hidden: 3,
    }));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let strip = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::QuoteFold(..)))
        .expect("a fold handle")
        .rect;
    assert_eq!(
        layout.hit_test(strip.x + 2, strip.y + strip.height / 2),
        Some(ActionId(7)),
        "the leading edge of the byline did not fold it"
    );
    assert_eq!(
        layout.hit_test(strip.x + strip.width - 2, strip.y + strip.height / 2),
        Some(ActionId(7)),
        "the trailing edge of the byline did not fold it"
    );
}

#[test]
fn a_shut_fold_says_how_much_is_behind_it_and_an_open_one_does_not() {
    let count = |collapsed| {
        a_byline(Some(Fold {
            action: ActionId(7),
            collapsed,
            hidden: 12,
        }))
        .layout_with(&CLARA_BW_METRICS, &Chrome::default())
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::QuoteFold(..)))
        .expect("a fold handle")
        .text_lines
        .clone()
    };
    assert_eq!(count(true), vec!["12".to_owned()]);
    assert!(
        count(false).is_empty(),
        "an open comment counted replies that are already on the page"
    );
}

fn with_an_overlay(kind: OverlayKind) -> Screen {
    Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(5),
            label: "Underneath".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    )
    .with_overlay(Overlay {
        id: NodeId(40),
        kind,
        title: "Delete this?".to_owned(),
        nodes: vec![Node::Button {
            id: NodeId(41),
            action: ActionId(6),
            label: "Delete".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Primary,
        }],
    })
}

/// The width of the overlay card on a Clara.
fn overlay_width(screen: &Screen) -> i32 {
    screen
        .layout_with(&CLARA_BW_METRICS, &Chrome::default())
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Overlay)
        .expect("an overlay")
        .rect
        .width
}

/// A popover holding a menu of rows, anchored to the button underneath.
fn with_a_menu(items: &[&str]) -> Screen {
    let mut screen = with_an_overlay(OverlayKind::Popover {
        anchor: ActionId(5),
    });
    let overlay = screen.overlay.as_mut().expect("an overlay");
    overlay.title = String::new();
    overlay.nodes = vec![Node::Rows {
        id: NodeId(41),
        rows: items
            .iter()
            .enumerate()
            .map(|(index, label)| Row {
                action: ActionId(6 + index as u32),
                title: (*label).to_owned(),
                summary: String::new(),
                lead: RowLead::Icon(Glyph::Trash),
                state: RowState::Open,
                trailing: None,
                menu: None,
            })
            .collect(),
    }];
    screen
}

/// The rect of the first row lead on a Clara.
fn first_lead(screen: &Screen) -> Rect {
    screen
        .layout_with(&CLARA_BW_METRICS, &Chrome::default())
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::RowLead(_)))
        .expect("a row lead")
        .rect
}

/// One row carrying the given lead.
fn row_with(lead: RowLead) -> Screen {
    Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![Row {
                action: ActionId(2),
                title: "A title".to_owned(),
                summary: String::new(),
                lead,
                state: RowState::Open,
                trailing: None,
                menu: None,
            }],
        }],
    )
}

/// One row whose summary is long enough to wrap, so the row is taller
/// than the title it leads.
fn row_with_wrapping_summary(lead: RowLead) -> Screen {
    Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![Row {
                action: ActionId(2),
                title: "A title".to_owned(),
                summary: "A sentence with quite enough words in it to run \
                              past the end of one line and onto a second, and \
                              very probably onto a third as well."
                    .to_owned(),
                lead,
                state: RowState::Open,
                trailing: None,
                menu: None,
            }],
        }],
    )
}

#[test]
fn a_row_mark_sits_against_the_title_it_marks() {
    for lead in [RowLead::Icon(Glyph::Circle), RowLead::Number(3)] {
        let screen = row_with_wrapping_summary(lead);
        let laid = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let mark = laid
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::RowLead(_)))
            .expect("a row lead")
            .rect;
        let title = laid
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::RowTitle))
            .expect("a row title")
            .rect;
        let middle = mark.y + mark.height / 2;
        // The summary runs to more than one line, so a mark centred on
        // the whole row would sink past the title and sit against the
        // sentence instead, reading as a mark on that.
        assert!(
            middle >= title.y && middle <= title.y + title.height,
            "{lead:?} sits at {middle}, outside the title at {} to {}",
            title.y,
            title.y + title.height
        );
    }
}

#[test]
fn a_cover_stays_centred_on_the_row_because_it_is_the_row() {
    let lead = RowLead::Picture(TilePicture::new(PictureHandle(7), 19, 30), Glyph::Book);
    let screen = row_with_wrapping_summary(lead);
    let laid = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let cover = laid
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::RowLead(_)))
        .expect("a row lead")
        .rect;
    let row = laid
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Row(_)))
        .expect("a row")
        .rect;
    // A cover is the row's content rather than a label on its title, so
    // it keeps the whole row's middle.
    let slack = (i64::from(row.height) - i64::from(cover.height)).abs() / 2 + 2;
    assert!(
        (i64::from(cover.y) - i64::from(row.y)).abs() <= slack,
        "a cover at {} is not centred on a row at {} of {}",
        cover.y,
        row.y,
        row.height
    );
}

#[test]
fn an_icon_leading_a_row_is_drawn_at_the_size_of_the_type() {
    // Drawn at the full width of its column an icon is about nine
    // millimetres tall beside a three and a half millimetre title, which
    // is what made a list of icons read as clip art next to its own text.
    let icon = first_lead(&row_with(RowLead::Icon(Glyph::Rss)));
    let title = CLARA_BW_METRICS.tenth_mm(FontSize::Body.tenth_mm());
    assert!(
        icon.height <= title * 3 / 2,
        "an icon {} tall leads a title {title} tall",
        icon.height
    );
}

#[test]
fn the_interface_is_not_set_larger_than_the_books_beside_it() {
    // The scale is bracketed, because the failure it came from was a slow
    // one: every size was individually defensible and the set of them read
    // as a children's book. The ceiling is the largest title a mainstream
    // platform ships, 5.3 mm on iOS. The floor is a phone's caption, 1.9 mm.
    // A reader who wants more has TextScale, which is what it is for.
    assert!(FontSize::Heading.tenth_mm() <= 55);
    assert!(FontSize::Caption.tenth_mm() >= 19);
    assert!(FontSize::Caption.tenth_mm() < FontSize::Body.tenth_mm());
    assert!(FontSize::Body.tenth_mm() < FontSize::Title.tenth_mm());
    assert!(FontSize::Title.tenth_mm() < FontSize::Heading.tenth_mm());
}

#[test]
fn the_large_text_setting_lands_where_the_default_used_to_be() {
    // The point of coming down is that it restores the range above. If the
    // default ever climbs back, Large stops being a step and starts being
    // the only readable setting again.
    let large = FontSize::Body.tenth_mm() * TextScale::Large.percent() / 100;
    assert!((34..=38).contains(&large), "large body is {large} tenths");
}

#[test]
fn a_rank_sits_on_the_first_line_of_its_title() {
    // Centred against the row, a rank beside a two line title floated a
    // whole line below the ranks either side of it, and a column of
    // numbers at four different heights is what made a numbered list look
    // unfinished.
    let screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![
                Row::new(
                    ActionId(1),
                    "A headline long enough that it has to wrap onto a second line \
                         before it is done",
                    "news.example.com",
                    RowLead::Number(1),
                ),
                Row::new(ActionId(2), "Short", "news.example.com", RowLead::Number(2)),
            ],
        }],
    );
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let pairs: Vec<(i32, i32)> = layout
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            LayoutKind::RowLead(RowLead::Number(_)) => Some((node.rect.y, node.rect.height)),
            _ => None,
        })
        .collect();
    let titles: Vec<i32> = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RowTitle))
        .map(|node| node.rect.y)
        .collect();
    assert_eq!(pairs.len(), 2);
    assert_eq!(titles.len(), 2);
    for ((rank_y, rank_height), title_y) in pairs.into_iter().zip(titles) {
        let bottom = rank_y + rank_height;
        let first_line = title_y + FontSize::Body.line_height();
        assert_eq!(
            bottom,
            first_line,
            "a rank sat {} from the foot of its title's first line",
            first_line - bottom
        );
    }
}

#[test]
fn a_cover_still_fills_the_column_it_leads_with() {
    // The size rule is about labels, not content. A cover is the row.
    let cover = first_lead(&row_with(RowLead::Picture(
        TilePicture {
            handle: PictureHandle(1),
            source: (60, 90),
        },
        Glyph::Book,
    )));
    assert_eq!(cover.width, CLARA_BW_METRICS.touch_target_default());
}

#[test]
fn a_mark_fills_the_column_a_list_of_marks_is_given() {
    // The column is sized to the mark rather than to a finger, so there is
    // nothing left over to centre the mark in. What matters is that it
    // starts at the margin: a gutter wider than its contents is a tenth of
    // this panel spent on nothing, which is what it was.
    let icon = first_lead(&row_with(RowLead::Icon(Glyph::Rss)));
    assert_eq!(icon.x, CLARA_BW_METRICS.screen_margin());
    assert_eq!(icon.width, row_mark_column(&CLARA_BW_METRICS));
    assert!(
        icon.width < CLARA_BW_METRICS.touch_target_default(),
        "a mark column of {} is still a finger wide",
        icon.width
    );
}

#[test]
fn a_list_with_a_cover_in_it_keeps_one_text_margin() {
    // A cover needs the wide column and a mark does not, but a list where
    // some titles start further left than others is worse than a wide
    // gutter, so one cover widens the column for every row in the list.
    let mixed = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![
                Row::new(
                    ActionId(1),
                    "With a cover",
                    "",
                    RowLead::Picture(
                        TilePicture {
                            handle: PictureHandle(1),
                            source: (60, 90),
                        },
                        Glyph::Book,
                    ),
                ),
                Row::new(ActionId(2), "With a mark", "", RowLead::Icon(Glyph::Book)),
            ],
        }],
    );
    let starts: Vec<i32> = mixed
        .layout_with(&CLARA_BW_METRICS, &Chrome::default())
        .nodes
        .iter()
        .filter(|node| node.kind == LayoutKind::RowTitle)
        .map(|node| node.rect.x)
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0], starts[1]);
    assert_eq!(
        starts[0],
        CLARA_BW_METRICS.screen_margin()
            + CLARA_BW_METRICS.touch_target_default()
            + CLARA_BW_METRICS.space(Space::Small)
    );
}

#[test]
fn a_list_of_marks_starts_its_text_at_the_mark_column() {
    let with_icon = row_with(RowLead::Icon(Glyph::Rss));
    let title = |screen: &Screen| {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::RowTitle)
            .expect("a title")
            .rect
    };
    let expected = CLARA_BW_METRICS.screen_margin()
        + row_mark_column(&CLARA_BW_METRICS)
        + CLARA_BW_METRICS.space(Space::Small);
    assert_eq!(title(&with_icon).x, expected);
}

#[test]
fn a_popover_is_only_as_wide_as_what_is_in_it() {
    // A menu of one short word used to be given the same width as a
    // dialogue, so pressing a three dot mark produced a band most of the
    // way across the panel and read as the screen having changed.
    let narrow = overlay_width(&with_a_menu(&["Delete"]));
    let wide = overlay_width(&with_a_menu(&[
        "Stop following this feed and forget everything it ever said",
    ]));
    assert!(
        narrow < wide,
        "a one word menu was as wide as a sentence: {narrow} against {wide}"
    );
    assert!(
        narrow < CLARA_BW_METRICS.width / 2,
        "a one word menu took half the panel: {narrow}"
    );
}

#[test]
fn a_deck_place_with_nothing_on_it_is_drawn_lighter_than_a_key() {
    // Fifteen places with three assigned used to be twelve boxes as heavy
    // as the three that did something, and every one of them looked as
    // pressable as the next.
    let cells: Vec<Cell> = (0..15)
        .map(|index| {
            let cell = Cell::new(ActionId(index + 1), if index < 3 { "Test" } else { "" });
            if index < 3 {
                cell.with_glyph(Glyph::Grid)
            } else {
                cell
            }
        })
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 5,
            square: true,
            cells,
        }],
    );
    let laid_out = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let assigned = laid_out
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Cell(_, CellStyle::Pad, _)))
        .count();
    let places = laid_out
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Cell(_, CellStyle::EmptyPad, _)))
        .count();
    assert_eq!((assigned, places), (3, 12));
}

#[test]
fn a_button_is_as_tall_as_the_words_on_it() {
    // Two secondary actions side by side give each label half the panel,
    // and at the larger reader text settings "Clear finished" needs two
    // lines of it. Drawn in a box one line tall, the renderer refuses the
    // whole screen, so a list that had a finished item on it went blank.
    let metrics = DisplayMetrics {
        text_scale: TextScale::Larger,
        ..CLARA_BW_METRICS
    };
    with_text_scale(TextScale::Larger, || {
        let screen = Screen::new(
            1,
            vec![Node::Band {
                id: NodeId(1),
                align: BandAlign::Middle,
                slots: vec![
                    BandSlot {
                        width: SlotWidth::Fill,
                        nodes: vec![Node::Button {
                            id: NodeId(2),
                            action: ActionId(2),
                            label: "Add".to_owned(),
                            state: ControlState::Enabled,
                            emphasis: Emphasis::Normal,
                        }],
                    },
                    BandSlot {
                        width: SlotWidth::Fill,
                        nodes: vec![Node::Button {
                            id: NodeId(3),
                            action: ActionId(3),
                            label: "Clear finished".to_owned(),
                            state: ControlState::Enabled,
                            emphasis: Emphasis::Normal,
                        }],
                    },
                ],
            }],
        );
        let laid_out = screen.layout_with(&metrics, &Chrome::default());
        let button = laid_out
            .nodes
            .iter()
            .find(|node| {
                node.kind
                    == LayoutKind::Button(ActionId(3), ControlState::Enabled, Emphasis::Normal)
            })
            .expect("the second button");
        let needed =
            i32::try_from(button.text_lines.len()).unwrap_or(1) * FontSize::Body.line_height();
        assert!(
            button.rect.height >= needed,
            "{} lines of label in {} pixels",
            button.text_lines.len(),
            button.rect.height
        );
        assert!(
            screen
                .diagnostics(&metrics, &Chrome::default())
                .issues
                .iter()
                .all(|issue| issue.severity != DiagnosticSeverity::Error),
            "the renderer would refuse this screen"
        );
    });
}

#[test]
fn a_dialogue_asks_its_whole_question_however_large_the_type_is() {
    // At 170% "Download Mrs Dalloway?" reached the reader as
    // "Download Mrs", because an overlay title was cut to one line
    // whatever it said. A menu header is a label and still is; a
    // dialogue's title is the question it exists to ask.
    let metrics = DisplayMetrics {
        text_scale: TextScale::Larger,
        ..CLARA_BW_METRICS
    };
    with_text_scale(TextScale::Larger, || {
        let mut screen = with_an_overlay(OverlayKind::Modal);
        screen.overlay.as_mut().expect("an overlay").title = "Download Mrs Dalloway?".to_owned();
        let lines = screen
            .layout_with(&metrics, &Chrome::default())
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::OverlayTitle)
            .expect("the title")
            .text_lines
            .join(" ");
        assert_eq!(lines, "Download Mrs Dalloway?");
    });
}

#[test]
fn a_menu_of_buttons_is_wide_enough_for_the_words_on_them() {
    // At the largest reader text setting a menu was measured with a
    // narrower padding than the buttons in it were drawn with, so the
    // label of every item wrapped inside a box built to hold it.
    with_text_scale(TextScale::Largest, || {
        let mut screen = with_an_overlay(OverlayKind::Popover {
            anchor: ActionId(5),
        });
        let overlay = screen.overlay.as_mut().expect("an overlay");
        overlay.title = String::new();
        overlay.nodes = vec![Node::Button {
            id: NodeId(41),
            action: ActionId(6),
            label: "Rename".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }];
        let metrics = DisplayMetrics {
            text_scale: TextScale::Largest,
            ..CLARA_BW_METRICS
        };
        let lines = screen
            .layout_with(&metrics, &Chrome::default())
            .nodes
            .iter()
            .find(|node| {
                node.kind
                    == LayoutKind::Button(ActionId(6), ControlState::Enabled, Emphasis::Normal)
            })
            .expect("the menu item")
            .text_lines
            .clone();
        assert_eq!(lines, vec!["Rename".to_owned()], "the label was broken up");
    });
}

#[test]
fn a_popover_never_gets_narrower_than_a_finger() {
    let width = overlay_width(&with_a_menu(&["No"]));
    assert!(
        width >= 3 * CLARA_BW_METRICS.touch_target_default(),
        "a two letter menu was {width}, too narrow to press or to carry a caret"
    );
}

#[test]
fn a_popover_never_gets_wider_than_a_modal() {
    let sentence = "Stop following this feed and forget everything it ever said, \
                        including the parts nobody read, at once and for good";
    let width = overlay_width(&with_a_menu(&[sentence]));
    let modal = overlay_width(&with_an_overlay(OverlayKind::Modal));
    assert_eq!(
        width, modal,
        "a long menu item pushed the popover past the width a modal is allowed"
    );
}

#[test]
fn a_modal_still_takes_the_room_a_dialogue_needs() {
    // Deliberately not measured from its contents. A dialogue asks a
    // question in prose, and prose set in a box the width of its longest
    // button is a column of two words.
    let terse = overlay_width(&with_an_overlay(OverlayKind::Modal));
    let mut wordy = with_an_overlay(OverlayKind::Modal);
    wordy.overlay.as_mut().expect("an overlay").title =
        "Delete this feed and everything in it?".to_owned();
    assert_eq!(terse, overlay_width(&wordy));
}

#[test]
fn an_overlay_leaves_the_screen_it_covers_alone() {
    // Dimming the backdrop is the usual way to focus attention on a
    // dialogue and the one thing this panel cannot afford: shading every
    // pixel forces a full refresh to put it up and a second one to take it
    // down. The overlay has to earn its separation with a border instead.
    let screen = with_an_overlay(OverlayKind::Modal);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let card = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Overlay)
        .expect("an overlay")
        .rect;
    let (covered, stride) = paint(&screen);
    let mut bare = screen.clone();
    bare.overlay = None;
    let (bare, _) = paint(&bare);
    // Everything above the card, pixel for pixel. Not "all paper": the
    // button underneath is drawn up there and has to still be drawn,
    // which is the whole claim.
    let above = card.y.max(1) as usize * stride;
    assert_eq!(
        covered.pixels[..above],
        bare.pixels[..above],
        "the screen under the overlay was altered, so putting the overlay up and \
             taking it down each cost a full refresh"
    );
}

#[test]
fn a_tap_beside_a_modal_answers_nothing() {
    // "Somewhere else" is not one of the choices, and a reader who brushes
    // the panel has not agreed to delete anything.
    let screen = with_an_overlay(OverlayKind::Modal);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert_eq!(
        layout.hit_test(2, CLARA_BW_METRICS.height - 2),
        None,
        "a tap outside a modal reached a control the reader could not see"
    );
}

#[test]
fn a_tap_beside_a_popover_puts_it_away() {
    let screen = Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(5),
            label: "More".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    )
    .with_overlay(Overlay::popover(
        NodeId(40),
        ActionId(5),
        vec![Node::Button {
            id: NodeId(41),
            action: ActionId(6),
            label: "Share".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert_eq!(
        layout.hit_test(2, CLARA_BW_METRICS.height - 2),
        Some(ActionId::BACK),
        "a tap outside a popover did not dismiss it"
    );
}

#[test]
fn a_popover_points_at_the_control_that_opened_it() {
    let screen = Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(5),
            label: "More".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    )
    .with_overlay(Overlay::popover(
        NodeId(40),
        ActionId(5),
        vec![Node::Button {
            id: NodeId(41),
            action: ActionId(6),
            label: "Share".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let anchor = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(ActionId(5), _, _)))
        .expect("the anchor")
        .rect;
    let caret = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::OverlayCaret(_)))
        .expect("a caret")
        .rect;
    let centre = caret.x + caret.width / 2;
    assert!(
        centre >= anchor.x && centre <= anchor.x + anchor.width,
        "the caret pointed at {centre}, which is not anywhere on the control at \
             {}..{}",
        anchor.x,
        anchor.x + anchor.width
    );
}

#[test]
fn an_overlay_with_nothing_to_point_at_is_still_shown() {
    // Naming an anchor that is not on the screen is an application bug.
    // Dropping the overlay loses whatever it was going to ask, which is a
    // worse answer than centring it.
    let screen = Screen::new(1, vec![])
        .with_overlay(Overlay::popover(NodeId(40), ActionId(999), vec![]))
        .with_overlay(Overlay::popover(
            NodeId(40),
            ActionId(999),
            vec![Node::Text {
                id: NodeId(41),
                text: "Something".to_owned(),
                links: Vec::new(),
            }],
        ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert!(
        layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::Overlay),
        "an overlay with a missing anchor was dropped"
    );
    assert!(
        !layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::OverlayCaret(_))),
        "a caret was drawn pointing at nothing"
    );
}

#[test]
fn an_overlay_takes_the_page_turns_away() {
    // The zones are whatever is left of the content area, and what is left
    // must not include the thing drawn on top of it: a reader answering a
    // question would otherwise turn the page underneath instead.
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "A page of a book.".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_page_turns(ActionId(2), ActionId(3))
    .with_overlay(Overlay::modal(
        NodeId(40),
        "Leave?",
        vec![Node::Button {
            id: NodeId(41),
            action: ActionId(6),
            label: "Leave".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Primary,
        }],
    ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert_eq!(layout.hit_page_turn(4, CLARA_BW_METRICS.height / 2), None);
    // Suppressed, and saying so: a screen that declared turns and is now
    // covered must not read as one that never declared any. The physical
    // page keys tell those apart, and paged the content underneath the
    // dialog while they could not.
    assert_eq!(layout.page_turns, PagingState::SuppressedByOverlay);
    let uncovered = Screen::new(1, Vec::new()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert_eq!(uncovered.page_turns, PagingState::None);
}

/// A modal is deliberately not dismissed by a tap that misses it, so
/// without a cross it is a screen with no way off it unless the
/// application remembered to put one in a row. The frame draws it, so the
/// application cannot forget.
#[test]
fn a_modal_always_carries_a_way_out() {
    for (name, metrics) in PANELS {
        for title in ["", "Delete this book?"] {
            let screen = Screen::new(1, Vec::new()).with_overlay(Overlay {
                kind: OverlayKind::Modal,
                id: NodeId(2),
                title: title.into(),
                nodes: vec![Node::Text {
                    id: NodeId(3),
                    text: "This cannot be undone.".into(),
                    links: Vec::new(),
                }],
            });
            let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
            let cross = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::OverlayClose)
                .unwrap_or_else(|| panic!("{name}: a modal with no way out"));
            assert_eq!(
                layout.hit_control(
                    cross.rect.x + cross.rect.width / 2,
                    cross.rect.y + cross.rect.height / 2
                ),
                Some(ActionId::BACK),
                "{name}: the cross does not answer"
            );
            assert_eq!(
                layout.pressed_control(
                    cross.rect.x + cross.rect.width / 2,
                    cross.rect.y + cross.rect.height / 2
                ),
                Some(cross.rect),
                "{name}: pressing the cross marked a control behind the modal"
            );
            let target = metrics.touch_target_default();
            assert!(
                cross.rect.width >= target && cross.rect.height >= target,
                "{name}: the cross is smaller than a finger"
            );
            // Under the cross rather than beside it is how a title ends up
            // unreadable, so the two never share pixels.
            if let Some(heading) = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::OverlayTitle)
            {
                assert!(
                    heading.rect.x + heading.rect.width <= cross.rect.x,
                    "{name}: the title runs under the cross"
                );
            }
        }
    }
}

/// A popover needs no cross: a tap anywhere off it puts it away, and a
/// second way out beside the control that opened it is noise.
#[test]
fn a_popover_carries_no_cross() {
    let screen = Screen::new(1, Vec::new())
        .with_top_bar(TopBar::new(NodeId(1), "Title").action(ActionId(9), "Aa"))
        .with_overlay(Overlay {
            kind: OverlayKind::Popover {
                anchor: ActionId(9),
            },
            id: NodeId(2),
            title: "Type size".into(),
            nodes: vec![Node::Text {
                id: NodeId(3),
                text: "Standard".into(),
                links: Vec::new(),
            }],
        });
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
    assert!(
        !layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::OverlayClose),
        "a popover drew a cross it does not need"
    );
}

/// A popover hung off a picture control has to find that control, and the
/// control has to stay pressable so a second tap puts the panel away.
///
/// Both failed together and neither was visible in the screen: a bar glyph
/// was added to the hit test and to the renderer but not to
/// `LayoutKind::acts_on`, so the anchor lookup came back empty, the
/// popover quietly fell back to a centred modal, and the modal's scrim sat
/// over the control that opened it. On the panel that read as a front
/// light menu in the middle of the page that could not be dismissed, while
/// the type menu beside it -- a word, not a picture -- worked.
#[test]
fn a_popover_hung_off_a_picture_control_finds_it_and_leaves_it_pressable() {
    let glyph = ActionId(31);
    let screen = Screen::new(1, Vec::new())
        .with_top_bar(
            TopBar::new(NodeId(1), "3 of 40")
                .with_action(BarAction::new(glyph, "Front light").with_glyph(Glyph::Light)),
        )
        .with_overlay(Overlay {
            kind: OverlayKind::Popover { anchor: glyph },
            id: NodeId(2),
            title: "Front light 20%".into(),
            nodes: vec![Node::Button {
                id: NodeId(3),
                action: ActionId(32),
                label: "Brighter".into(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }],
        });
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
    let anchor = layout
        .rect_of_action(glyph)
        .expect("the control the panel hangs off");
    let box_rect = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Overlay)
        .map(|node| node.rect)
        .expect("the panel");
    // Under the control it belongs to, not adrift in the middle of the
    // page, which is what a silent fall back to a modal looks like.
    assert!(
        box_rect.y >= anchor.y + anchor.height,
        "the panel did not hang off its control: anchor {anchor:?}, panel {box_rect:?}"
    );
    assert!(
        layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::OverlayCaret(_))),
        "a panel that hangs off a control draws the mark that says so"
    );
    // A tap on the control that opened it lands on the scrim, and a
    // popover's scrim answers a miss with Back, which is what puts it
    // away. That is the whole mechanism, and it is the half the fall back
    // to a modal silently disabled -- a modal is deliberately *not*
    // dismissed by a miss, so the panel became something that could only
    // be left by tapping one of its own rows.
    assert_eq!(
        layout.hit_control(anchor.x + anchor.width / 2, anchor.y + anchor.height / 2),
        Some(ActionId::BACK),
        "a tap on the control that opened the panel does not put it away"
    );
}

#[test]
fn a_popover_too_tall_for_either_side_draws_no_caret() {
    // On a Clara the reading panel could not fit below its control in the
    // top bar or above it, so it was clamped into the middle of the screen
    // -- covering the control -- and still drew a caret, at the far bottom
    // corner, pointing down at the page. A mark that points at nothing is
    // read as pointing at whatever is nearest.
    let tall = (0..14)
        .map(|index| Node::Button {
            id: NodeId(100 + index),
            action: ActionId(100 + index),
            label: format!("Choice {index}"),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        })
        .collect::<Vec<_>>();
    let screen = Screen::new(1, Vec::new())
        .with_top_bar(TopBar::new(NodeId(1), "Title").action(ActionId(9), "Aa"))
        .with_overlay(Overlay::popover(NodeId(40), ActionId(9), tall));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    let anchor = layout
        .rect_of_action(ActionId(9))
        .expect("the control the panel hangs off");
    let box_rect = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Overlay)
        .map(|node| node.rect)
        .expect("the panel");
    let caret = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::OverlayCaret(_)));
    let overlaps = box_rect.y < anchor.y + anchor.height && anchor.y < box_rect.y + box_rect.height;
    assert!(overlaps, "this case is only interesting when they overlap");
    assert!(
        caret.is_none(),
        "a caret was drawn on a panel that covers its own anchor"
    );
}

#[test]
fn a_hold_is_the_content_area_and_never_a_control() {
    // A reader resting a thumb on a button must get the button. The whole
    // point of the gesture is that a page of prose has nowhere else to put
    // one, which is an argument about empty space, not about controls.
    let screen = Screen::new(
        1,
        vec![
            Node::Text {
                id: NodeId(1),
                text: "A page of a book.".to_owned(),
                links: Vec::new(),
            },
            Node::Button {
                id: NodeId(2),
                action: ActionId(7),
                label: "Notes".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            },
        ],
    )
    .with_hold(ActionId(9));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let button = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(ActionId(7), _, _)))
        .expect("the button");
    assert_eq!(
        layout.hit_hold(
            button.rect.x + button.rect.width / 2,
            button.rect.y + button.rect.height / 2
        ),
        None,
        "holding a control was taken as a hold on the page"
    );
    assert_eq!(
        layout.hit_hold(CLARA_BW_METRICS.width / 2, layout.content.y + 4),
        Some(ActionId(9))
    );
    // Above the content is the top bar's business, not the page's.
    assert_eq!(
        layout.hit_hold(CLARA_BW_METRICS.width / 2, layout.content.y - 1),
        None
    );
}

#[test]
fn an_overlay_takes_the_hold_away() {
    // Same argument as the page turns: what is left over must not include
    // what is drawn on top of it, or holding a finger on a panel would
    // reach through it into the book.
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "A page of a book.".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_hold(ActionId(9))
    .with_overlay(Overlay::popover(
        NodeId(40),
        ActionId(999),
        vec![Node::Text {
            id: NodeId(41),
            text: "Type size".to_owned(),
            links: Vec::new(),
        }],
    ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    assert_eq!(
        layout.hit_hold(CLARA_BW_METRICS.width / 2, layout.content.y + 4),
        None
    );
}

#[test]
fn a_row_cover_falls_back_to_its_glyph_until_the_picture_arrives() {
    let screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![Row::new(
                ActionId(1),
                "Bleak House",
                "Charles Dickens",
                RowLead::Picture(TilePicture::new(PictureHandle(7), 190, 300), Glyph::Book),
            )],
        }],
    );
    let lead = screen
        .layout()
        .nodes
        .into_iter()
        .find(|node| matches!(node.kind, LayoutKind::RowLead(_)))
        .expect("a row lead");
    let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
    let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
    let inked = |cache: &PictureCache| {
        let mut surface = Surface::new(stride, height);
        surface.clear(tone::PAPER);
        render_all(
            &screen,
            &CLARA_BW_METRICS,
            &Chrome::with_back(false),
            cache,
            &mut surface,
            None,
        );
        (lead.rect.y..lead.rect.y + lead.rect.height)
            .flat_map(|row| {
                (lead.rect.x..lead.rect.x + lead.rect.width).map(move |column| (column, row))
            })
            .filter(|&(column, row)| {
                let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
                    return false;
                };
                surface.pixels[row * stride + column] != tone::PAPER
            })
            .count()
    };
    // Nothing in the cache: the glyph still draws, so the row is usable
    // while the covers are on their way.
    let empty = PictureCache::default();
    assert!(inked(&empty) > 0, "a row with no cover yet drew nothing");
    let mut cache = PictureCache::default();
    assert!(cache.put(PictureHandle(7), 19, 30, vec![0; 19 * 30]));
    assert!(
        inked(&cache) > inked(&empty),
        "the cover was not drawn once it had arrived"
    );
}

#[test]
fn a_screen_with_no_bottom_bar_still_keeps_the_bezel_margin() {
    // The sides get a screen margin and the bottom got nothing, so the
    // last control on a screen with no bar under it was drawn onto the
    // final rows of the panel: on the reader the Add button's border was
    // the bottom edge of the glass.
    let screen = Screen::new(
        1,
        vec![
            Node::Text {
                id: NodeId(1),
                text: "Nothing to do.".into(),
                links: Vec::new(),
            },
            Node::Button {
                id: NodeId(2),
                action: ActionId(1),
                label: "Add".into(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            },
        ],
    )
    .with_top_bar(TopBar::new(NodeId(0), "Todo"));
    let layout = screen.layout();
    let floor = CLARA_BW_METRICS.height - CLARA_BW_METRICS.screen_margin();
    for node in &layout.nodes {
        assert!(
            node.rect.y + node.rect.height <= floor,
            "{:?} ran to {}, past the bezel margin at {floor}",
            node.kind,
            node.rect.y + node.rect.height
        );
    }
}

#[test]
fn a_page_position_is_drawn_and_takes_its_room_from_the_content() {
    let build = |position: Option<(u16, u16)>| {
        let mut screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "A page of prose.".into(),
                links: Vec::new(),
            }],
        )
        .with_page_turns(ActionId(1), ActionId(2));
        if let Some((page, of)) = position {
            screen.page_turns = screen.page_turns.map(|turns| turns.with_position(page, of));
        }
        screen.layout()
    };
    let without = build(None);
    let with = build(Some((4, 12)));
    let shown = with
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::PagePosition)
        .expect("a page position");
    assert_eq!(shown.text_lines[0], "4 of 12");
    assert!(
        !without
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::PagePosition),
        "a screen that asked for no position got one anyway"
    );
    assert!(
        with.content.height < without.content.height,
        "the position band was drawn over the content instead of reserved"
    );
    assert!(
        shown.rect.y >= with.content.y + with.content.height,
        "the position was drawn inside the page-turn zone it describes"
    );
}

#[test]
fn each_page_turn_is_offered_only_where_there_is_a_page_to_turn_to() {
    // The side-tap zones are invisible, so a paginated screen used to say
    // which page it was on and nothing about how to leave it.
    let turns = |page: u16| {
        let mut screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "A page of prose.".into(),
                links: Vec::new(),
            }],
        )
        .with_page_turns(ActionId(7), ActionId(9));
        screen.page_turns = screen.page_turns.map(|turns| turns.with_position(page, 3));
        let layout = screen.layout();
        let seen: Vec<LayoutKind> = layout
            .nodes
            .iter()
            .map(|node| node.kind)
            .filter(|kind| matches!(kind, LayoutKind::PagePrevious(_) | LayoutKind::PageNext(_)))
            .collect();
        (layout, seen)
    };
    let (_, first) = turns(1);
    assert_eq!(first, vec![LayoutKind::PageNext(ActionId(9))]);
    let (middle_layout, middle) = turns(2);
    assert_eq!(
        middle,
        vec![
            LayoutKind::PagePrevious(ActionId(7)),
            LayoutKind::PageNext(ActionId(9))
        ]
    );
    let (_, last) = turns(3);
    assert_eq!(last, vec![LayoutKind::PagePrevious(ActionId(7))]);

    // And they are controls, not decoration: a tap lands on the action
    // rather than falling through to the zone underneath.
    let previous = middle_layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::PagePrevious(_)))
        .expect("a previous control");
    assert_eq!(
        middle_layout.hit_test(
            previous.rect.x + previous.rect.width / 2,
            previous.rect.y + previous.rect.height / 2,
        ),
        Some(ActionId(7))
    );
    assert!(
        previous.rect.height >= DisplayMetrics::default().touch_target_minimum(),
        "the page controls are too small to hit: {:?}",
        previous.rect
    );
}

#[test]
fn a_page_position_nobody_can_compute_is_left_unsaid() {
    for position in [(0, 0), (1, 0), (13, 12)] {
        let mut screen = Screen::new(1, Vec::new()).with_page_turns(ActionId(1), ActionId(2));
        screen.page_turns = screen
            .page_turns
            .map(|turns| turns.with_position(position.0, position.1));
        assert!(
            !screen
                .layout()
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::PagePosition),
            "{position:?} was drawn rather than left unsaid"
        );
    }
}

#[test]
fn an_empty_field_shows_its_placeholder_muted_and_a_filled_one_shows_ink() {
    let laid_out = |value: &str| {
        Screen::new(
            1,
            vec![Node::Field {
                id: NodeId(1),
                action: ActionId(1),
                value: value.into(),
                placeholder: "Search the library".into(),
                clear: None,
            }],
        )
        .layout()
    };
    let empty = laid_out("");
    let filled = laid_out("dickens");
    let value = |layout: &Layout| {
        layout
            .nodes
            .iter()
            .find_map(|node| match node.kind {
                LayoutKind::FieldValue(empty) => Some((empty, node.text_lines[0].clone())),
                _ => None,
            })
            .expect("a field value")
    };
    assert_eq!(value(&empty), (true, "Search the library".to_owned()));
    assert_eq!(value(&filled), (false, "dickens".to_owned()));
}

#[test]
fn a_field_cross_never_overlaps_the_text_it_would_clear() {
    let layout = Screen::new(
        1,
        vec![Node::Field {
            id: NodeId(1),
            action: ActionId(1),
            value: "a query long enough to run the whole width of the panel".into(),
            placeholder: String::new(),
            clear: Some(ActionId(2)),
        }],
    )
    .layout();
    let find = |wanted: fn(LayoutKind) -> bool| {
        layout
            .nodes
            .iter()
            .find(|node| wanted(node.kind))
            .expect("a field part")
            .rect
    };
    let value = find(|kind| matches!(kind, LayoutKind::FieldValue(_)));
    let clear = find(|kind| matches!(kind, LayoutKind::FieldClear(_)));
    assert!(
        value.x + value.width <= clear.x,
        "the query ran underneath the cross that clears it"
    );
    assert_eq!(
        layout.hit_test(clear.x + clear.width / 2, clear.y + clear.height / 2),
        Some(ActionId(2)),
        "the cross did not answer a tap"
    );
}

#[test]
fn chips_wrap_onto_another_row_rather_than_off_the_panel() {
    let chips: Vec<Chip> = (0..MAX_CHIPS)
        .map(|index| {
            Chip::new(
                ActionId(index as u32 + 1),
                format!("Subject number {index}"),
            )
        })
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Chips {
            id: NodeId(1),
            chips,
        }],
    );
    let layout = screen.layout();
    let placed: Vec<_> = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Chip(..)))
        .map(|node| node.rect)
        .collect();
    assert_eq!(placed.len(), MAX_CHIPS, "chips went missing");
    for chip in &placed {
        assert!(
            chip.x >= layout.content.x
                && chip.x + chip.width <= layout.content.x + layout.content.width,
            "a chip ran off the side of the panel"
        );
    }
    let rows = placed
        .iter()
        .map(|rect| rect.y)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(rows.len() > 1, "sixteen chips were crammed onto one row");
}

#[test]
fn chips_past_the_cap_are_reported_rather_than_quietly_clipped() {
    let chips: Vec<Chip> = (0..=MAX_CHIPS)
        .map(|index| Chip::new(ActionId(index as u32 + 1), "Subject"))
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Chips {
            id: NodeId(1),
            chips,
        }],
    );
    assert!(
        screen
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| matches!(issue.kind, LayoutIssueKind::CollectionTruncated { .. })),
        "a chip run past its cap was silently truncated"
    );
}

#[test]
fn a_tab_strip_fills_the_panel_exactly_and_marks_one_tab() {
    let screen = Screen::new(
        1,
        vec![Node::Tabs {
            id: NodeId(1),
            tabs: vec![
                Chip::new(ActionId(1), "Discover"),
                Chip::new(ActionId(2), "Popular"),
                Chip::new(ActionId(3), "Subjects"),
            ],
            selected: 2,
        }],
    );
    let layout = screen.layout();
    let tabs: Vec<_> = layout
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            LayoutKind::Tab(action, selected) => Some((action, selected, node.rect)),
            _ => None,
        })
        .collect();
    assert_eq!(tabs.len(), 3);
    // Measured against a full-width node on the same panel rather than
    // against the content rect, because the strip is placed inside the
    // screen margin and the content rect is outside it.
    let rule = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::TabRule)
        .expect("a strip rule")
        .rect;
    let first = tabs[0].2;
    let last = tabs[2].2;
    assert_eq!(first.x, rule.x, "the strip did not start at the margin");
    assert_eq!(
        last.x + last.width,
        rule.x + rule.width,
        "the rounding remainder left a gap at the end of the strip"
    );
    assert_eq!(
        tabs[1].2.x,
        first.x + first.width,
        "the tabs were not laid edge to edge"
    );
    let marked: Vec<_> = tabs.iter().filter(|(_, selected, _)| *selected).collect();
    assert_eq!(marked.len(), 1, "exactly one tab must ever be current");
    assert_eq!(marked[0].0, ActionId(3));
}

#[test]
fn a_tab_selection_nobody_named_falls_back_to_the_first() {
    let screen = Screen::new(
        1,
        vec![Node::Tabs {
            id: NodeId(1),
            tabs: vec![Chip::new(ActionId(1), "One"), Chip::new(ActionId(2), "Two")],
            selected: 99,
        }],
    );
    let marked = screen
        .layout()
        .nodes
        .into_iter()
        .find_map(|node| match node.kind {
            LayoutKind::Tab(action, true) => Some(action),
            _ => None,
        })
        .expect("a current tab");
    assert_eq!(
        marked,
        ActionId(1),
        "an out-of-range selection left the strip with no current tab"
    );
}

#[test]
fn an_unavailable_tile_keeps_its_place_and_stops_answering_taps() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            shape: TileShape::Square,
            tiles: vec![
                Tile::new(ActionId(1), "Here", Glyph::Book),
                Tile::new(ActionId(2), "Gone", Glyph::Book).with_state(TileState::Unavailable),
            ],
        }],
    );
    let layout = screen.layout();
    let cells: Vec<_> = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
        .collect();
    assert_eq!(cells.len(), 2, "an unavailable tile lost its place");
    let gone = cells[1].rect;
    assert_eq!(
        layout.hit_test(gone.x + gone.width / 2, gone.y + gone.height / 2),
        None,
        "an unavailable tile answered a tap"
    );
    let here = cells[0].rect;
    assert_eq!(
        layout.hit_test(here.x + here.width / 2, here.y + here.height / 2),
        Some(ActionId(1)),
        "an available tile beside it stopped answering too"
    );
}

#[test]
fn one_subtitle_gives_every_tile_in_the_grid_the_same_extra_room() {
    let heights = |subtitled: bool| {
        let mut second = Tile::new(ActionId(2), "Second", Glyph::Book);
        if subtitled {
            second = second.with_subtitle("Charles Dickens");
        }
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape: TileShape::Square,
                tiles: vec![Tile::new(ActionId(1), "First", Glyph::Book), second],
            }],
        );
        let layout = screen.layout();
        layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
            .map(|node| node.rect.height)
            .collect::<Vec<_>>()
    };
    let plain = heights(false);
    let tall = heights(true);
    assert_eq!(plain[0], plain[1], "a plain grid was already ragged");
    assert_eq!(
        tall[0], tall[1],
        "one subtitle made the grid ragged: cells in a grid are the same height"
    );
    assert!(
        tall[0] > plain[0],
        "a subtitle was given no room to be drawn in"
    );
}

#[test]
fn a_state_and_a_badge_take_opposite_corners_of_their_own_tile() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            shape: TileShape::Square,
            tiles: vec![Tile::new(ActionId(1), "Bleak House", Glyph::Book)
                .with_state(TileState::Held)
                .with_badge("12")],
        }],
    );
    let layout = screen.layout();
    let find = |wanted: fn(LayoutKind) -> bool| {
        layout
            .nodes
            .iter()
            .find(|node| wanted(node.kind))
            .expect("a corner chip")
            .rect
    };
    let cell = find(|kind| matches!(kind, LayoutKind::Tile(..)));
    let state = find(|kind| matches!(kind, LayoutKind::TileState(_)));
    let badge = find(|kind| kind == LayoutKind::TileBadge);
    assert!(
        badge.x < state.x,
        "the badge and the state marker piled into the same corner"
    );
    for chip in [state, badge] {
        assert!(
            chip.x >= cell.x
                && chip.y >= cell.y
                && chip.x + chip.width <= cell.x + cell.width
                && chip.y + chip.height <= cell.y + cell.height,
            "a corner chip hung outside its own tile"
        );
    }
}

#[test]
fn a_badge_is_clamped_rather_than_allowed_to_become_a_label() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            shape: TileShape::Square,
            tiles: vec![
                Tile::new(ActionId(1), "Shelf", Glyph::Book).with_badge("far too many characters")
            ],
        }],
    );
    let badge = screen
        .layout()
        .nodes
        .into_iter()
        .find(|node| node.kind == LayoutKind::TileBadge)
        .expect("a badge");
    assert_eq!(
        badge.text_lines[0].chars().count(),
        TILE_BADGE_LIMIT,
        "a badge was allowed to grow into a label"
    );
}

#[test]
fn a_document_preview_is_a_page_of_lines_with_a_format_chip() {
    let width = 180;
    let height = 280;
    let grey = document_preview(
        "The rain started before the kettle boiled.\nI left the window open.",
        "md",
        width,
        height,
    );
    assert_eq!(grey.len(), (width * height) as usize);
    assert!(
        grey.contains(&tone::INK),
        "the first lines left no ink on the page"
    );
    let w = width as usize;
    let h = height as usize;
    let foot = &grey[(h - 24) * w..];
    assert!(
        foot.contains(&tone::INK),
        "the format chip was not drawn in the trailing foot"
    );
    let with_lines = document_preview("A line of prose that should appear.", "md", width, height);
    let no_lines = document_preview("", "md", width, height);
    assert_ne!(
        with_lines, no_lines,
        "a preview without lines was indistinguishable from one with them"
    );
}

/// A square tile that refuses a tap has to show the mark that says so.
///
/// Outlining the square shape brought every unavailable square tile into
/// the branch that mutes a card, which drops the chip, leaving a tile that
/// looks ordinary, ignores taps and explains nothing. A muted card is a
/// different case: a whole grid of them is explained once by the screen.
#[test]
fn an_unavailable_square_tile_still_shows_its_mark() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            shape: TileShape::Square,
            tiles: vec![
                Tile::new(ActionId(1), "Gone", Glyph::Book).with_state(TileState::Unavailable)
            ],
        }],
    );
    assert!(
        screen
            .layout()
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::TileState(TileState::Unavailable))),
        "an unavailable square tile drew nothing to say it was unavailable"
    );
}

/// Only the card shape is outlined, and the square shape is not.
///
/// A tile outline is drawn in ink at the button weight rather than in grey
/// at the rule weight, so one reads as a hairline and a grid of them reads
/// as a box around every cell. The launcher is a grid of square tiles, and
/// it once shipped with a box around every app on it. A card is the other
/// case: they arrive a few at a time, and the outline is what separates one
/// card's worth of text from the next.
#[test]
fn only_a_card_is_outlined_and_a_square_tile_is_not() {
    let outlines = |shape| {
        Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape,
                tiles: vec![Tile::new(ActionId(1), "Read", Glyph::Book)],
            }],
        )
        .layout()
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::TileOutline(_)))
        .count()
    };
    assert_eq!(
        outlines(TileShape::Square),
        0,
        "a square tile drew an outline, which boxes every app on the launcher"
    );
    assert!(
        outlines(TileShape::Card) > 0,
        "a card lost the outline that separates it from the next card"
    );
}

#[test]
fn a_tile_with_nothing_extra_to_say_emits_no_chips_at_all() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            shape: TileShape::Square,
            tiles: vec![Tile::new(ActionId(1), "Plain", Glyph::Book)],
        }],
    );
    assert!(
        !screen.layout().nodes.iter().any(|node| matches!(
            node.kind,
            LayoutKind::TileState(_) | LayoutKind::TileBadge | LayoutKind::TileSubtitle
        )),
        "an unadorned tile still paid for decoration it did not ask for"
    );
}

#[test]
fn pagination_leaves_room_for_a_trailing_value_it_will_have_to_draw() {
    let metrics = CLARA_BW_METRICS;
    let area = ProseArea {
        width: 900,
        height: 900,
        gap: metrics.space(Space::Small),
        face: Face::Text,
    };
    let title = "A long headline about a small program that reads the news slowly \
                     and turns its pages by itself";
    let summary = "example.com, 4 hours ago, 12 comments";
    let plain = vec![(title, summary); 12];
    let scored = vec![(title, summary, "1,284 points and 312 comments"); 12];
    let without = paginate_rows(&plain, &metrics, area);
    let with = paginate_rows_with_trailing(&scored, &metrics, area);
    assert!(
        with[0].len() < without[0].len(),
        "the score took no room from the page: {} rows either way",
        with[0].len()
    );
    // What the row will really be: the same measurement the layout engine
    // makes, which is the thing the old pagination disagreed with.
    assert!(
        measured_row_height(
            &metrics,
            area,
            title,
            summary,
            "1,284 points and 312 comments",
            false,
            row_mark_column(&metrics)
        ) > measured_row_height(
            &metrics,
            area,
            title,
            summary,
            "",
            false,
            row_mark_column(&metrics)
        ),
        "a row with a value at its trailing edge measured no taller"
    );
}

#[test]
fn a_rows_trailing_value_keeps_its_room_and_the_title_gives_up_its_own() {
    let laid_out = |title: &str, trailing: Option<&str>| {
        let mut row = Row::new(ActionId(1), title, "Charles Dickens", Glyph::Book);
        if let Some(value) = trailing {
            row = row.with_trailing(value);
        }
        let screen = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![row],
            }],
        );
        screen.layout()
    };
    let long = "A title long enough that it would happily take the whole row";
    let without = laid_out(long, None);
    let with = laid_out(long, Some("18,204"));
    let title_width = |layout: &Layout| {
        layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::RowTitle)
            .expect("a title")
            .rect
            .width
    };
    assert!(
        title_width(&with) < title_width(&without),
        "a trailing value did not take any room from the title"
    );
    let value = with
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::RowTrailing)
        .expect("a trailing value");
    assert_eq!(
        value.text_lines.first().map(String::as_str),
        Some("18,204"),
        "the trailing value was clamped away"
    );
    let title = with
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::RowTitle)
        .expect("a title");
    assert!(
        title.rect.x + title.rect.width <= value.rect.x,
        "the title ran underneath its own trailing value"
    );
}

#[test]
fn an_empty_trailing_value_draws_nothing_at_all() {
    let screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![Row::new(ActionId(1), "Counter", "", Glyph::Note).with_trailing("")],
        }],
    );
    assert!(
        !screen
            .layout()
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::RowTrailing),
        "an empty trailing value still reserved a column"
    );
}

/// A tap on the dots must never also be a tap on the row. The two are the
/// same rectangle otherwise, and opening a feed while asking to remove it
/// is the worst possible reading of one press.
#[test]
fn a_row_menu_takes_the_tap_before_the_row_it_sits_in() {
    let screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![
                Row::new(ActionId(1), "Ars Technica", "arstechnica.com", Glyph::Rss)
                    .with_menu(ActionId(2)),
            ],
        }],
    );
    let layout = screen.layout();
    let mark = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::RowMenu(ActionId(2)))
        .expect("an overflow mark");
    let row = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Row(ActionId(1)))
        .expect("a row");
    assert_eq!(
        layout.hit_test(
            mark.rect.x + mark.rect.width / 2,
            mark.rect.y + mark.rect.height / 2
        ),
        Some(ActionId(2)),
        "the row swallowed a tap meant for its own overflow mark"
    );
    assert_eq!(
        layout.hit_test(row.rect.x + 10, row.rect.y + row.rect.height / 2),
        Some(ActionId(1)),
        "the mark swallowed a tap on the row"
    );
    assert_eq!(
        layout.pressed_control(
            mark.rect.x + mark.rect.width / 2,
            mark.rect.y + mark.rect.height / 2
        ),
        Some(mark.rect),
        "pressing the mark inverted the whole row"
    );
}

/// The mark keeps its column and the title wraps into what is left, on the
/// same reasoning as a trailing value: a title measured at the full row
/// width is drawn under the dots.
#[test]
fn a_row_menu_takes_its_room_from_the_title() {
    let long = "A headline long enough that it has to wrap somewhere on this panel";
    let laid_out = |menu: bool| {
        let row = Row::new(ActionId(1), long, "", Glyph::Rss);
        Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![if menu {
                    row.with_menu(ActionId(2))
                } else {
                    row
                }],
            }],
        )
        .layout()
    };
    let title = |layout: &Layout| {
        *layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::RowTitle)
            .map(|node| &node.rect)
            .expect("a title")
    };
    let with = laid_out(true);
    assert!(
        title(&with).width < title(&laid_out(false)).width,
        "an overflow mark did not take any room from the title"
    );
    let mark = with
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::RowMenu(_)))
        .expect("an overflow mark");
    let title = title(&with);
    assert!(
        title.x + title.width <= mark.rect.x,
        "the title ran underneath its own overflow mark"
    );
    assert!(
        mark.rect.width >= CLARA_BW_METRICS.touch_target_default(),
        "the overflow mark was smaller than a finger"
    );
}

/// Pagination that measures a row at the full width fits one line more per
/// row than the row will get, which puts the last row under the bottom bar.
#[test]
fn pagination_reserves_the_overflow_column() {
    let area = CLARA_BW_METRICS.prose_area(true, true);
    let full = row_title_width(&CLARA_BW_METRICS, area, "", false);
    assert_eq!(
        row_title_width(&CLARA_BW_METRICS, area, "", true),
        full - CLARA_BW_METRICS.touch_target_default(),
        "the measured title width did not give up a finger to the mark"
    );
    // A title that fits one line at the full width and needs two beside a
    // mark, found by measurement rather than picked by eye, so this stays
    // true if the face or the panel changes.
    let words = "feed ".repeat(40);
    let title = (1..=words.len())
        .filter(|end| words.is_char_boundary(*end))
        .map(|end| words[..end].trim_end().to_owned())
        .find(|candidate| {
            wrap_text(candidate, full, FontSize::Body).len() == 1
                && wrap_text(
                    candidate,
                    full - CLARA_BW_METRICS.touch_target_default(),
                    FontSize::Body,
                )
                .len()
                    == 2
        })
        .expect("a title that wraps only once the mark takes its column");
    assert!(
        measured_row_height(
            &CLARA_BW_METRICS,
            area,
            &title,
            "a summary",
            "",
            true,
            row_mark_column(&CLARA_BW_METRICS)
        ) > measured_row_height(
            &CLARA_BW_METRICS,
            area,
            &title,
            "a summary",
            "",
            false,
            row_mark_column(&CLARA_BW_METRICS)
        ),
        "the overflow column cost the title nothing"
    );
    let summary = "a summary that is itself long enough to wrap onto a second line here";
    let rows = vec![(&title[..], summary); 40];
    let with = paginate_rows_with_menu(&rows, &CLARA_BW_METRICS, area);
    let without = paginate_rows(&rows, &CLARA_BW_METRICS, area);
    assert!(
        with.first().map_or(0, Vec::len) < without.first().map_or(0, Vec::len),
        "a list with overflow marks paginated as though it had none"
    );
}

#[test]
fn a_row_without_a_menu_reserves_no_column_for_one() {
    let screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![Row::new(ActionId(1), "Counter", "", Glyph::Note)],
        }],
    );
    assert!(
        !screen
            .layout()
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::RowMenu(_))),
        "a row with no overflow action still drew one"
    );
}

#[test]
fn every_fact_value_starts_in_the_same_column() {
    let screen = Screen::new(
        1,
        vec![Node::Facts {
            id: NodeId(1),
            entries: vec![
                ("Downloads".to_owned(), "94,206".to_owned()),
                ("A much longer label".to_owned(), "2701".to_owned()),
                ("ID".to_owned(), "Public domain in the USA".to_owned()),
            ],
        }],
    );
    let layout = screen.layout();
    let values = layout
        .nodes
        .iter()
        .filter(|node| node.kind == LayoutKind::FactValue)
        .map(|node| node.rect.x)
        .collect::<Vec<_>>();
    assert_eq!(values.len(), 3, "a fact was dropped");
    assert!(
        values.iter().all(|x| *x == values[0]),
        "fact values did not share one column: {values:?}"
    );
}

#[test]
fn one_long_label_cannot_squeeze_every_value_into_a_gutter() {
    let screen = Screen::new(
        1,
        vec![Node::Facts {
            id: NodeId(1),
            entries: vec![(
                "A label so long that it would take the whole panel if it were allowed to"
                    .to_owned(),
                "94,206".to_owned(),
            )],
        }],
    );
    let layout = screen.layout();
    let value = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::FactValue)
        .expect("laid out");
    assert!(
        value.rect.width * 2 > layout.content.width,
        "the label column took more than half the panel from its value"
    );
}

#[test]
fn facts_that_run_out_of_panel_drop_whole_entries_instead_of_clipping() {
    let entries = (0..20)
        .map(|index| {
            (
                format!("Label {index}"),
                format!("A value with enough words to wrap onto a second line {index}"),
            )
        })
        .collect::<Vec<_>>();
    let screen = Screen::new(
        1,
        vec![Node::Facts {
            id: NodeId(1),
            entries: entries.clone(),
        }],
    );
    let issues = screen.validate(&CLARA_BW_METRICS);
    assert!(
        !issues.iter().any(|issue| matches!(
            issue.kind,
            LayoutIssueKind::Clipped | LayoutIssueKind::TextOverflow
        )),
        "a facts block taller than the panel clipped: {issues:?}"
    );
    let shown = screen
        .layout()
        .nodes
        .iter()
        .filter(|node| node.kind == LayoutKind::FactValue)
        .count();
    assert!(
        shown < entries.len(),
        "every fact was laid out on a panel that cannot hold them"
    );
    assert!(shown > 0, "no facts were laid out at all");
}

#[test]
fn facts_past_the_cap_are_reported_rather_than_quietly_clipped() {
    let entries = (0..MAX_FACTS + 4)
        .map(|index| (format!("Label {index}"), format!("{index}")))
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Facts {
            id: NodeId(1),
            entries,
        }],
    );
    let issues = screen.validate(&CLARA_BW_METRICS);
    assert!(
        issues.iter().any(|issue| matches!(
            issue.kind,
            LayoutIssueKind::CollectionTruncated {
                collection: "facts",
                ..
            }
        )),
        "a facts block over the cap reported nothing: {issues:?}"
    );
}

#[test]
fn a_band_puts_its_slots_beside_each_other() {
    let screen = Screen::new(
        1,
        vec![Node::Band {
            id: NodeId(1),
            align: BandAlign::Top,
            slots: vec![
                BandSlot::fill(vec![Node::Text {
                    id: NodeId(2),
                    text: "Left".to_owned(),
                    links: Vec::new(),
                }]),
                BandSlot::fill(vec![Node::Text {
                    id: NodeId(3),
                    text: "Right".to_owned(),
                    links: Vec::new(),
                }]),
            ],
        }],
    );
    let layout = screen.layout();
    let rect = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("laid out")
            .rect
    };
    assert_eq!(rect(2).y, rect(3).y, "slots did not share a top edge");
    assert!(
        rect(3).x > rect(2).x + rect(2).width,
        "the second slot overlapped the first instead of sitting beside it"
    );
}

#[test]
fn a_band_too_narrow_for_its_slots_stacks_itself() {
    let narrow = DisplayMetrics {
        width: metrics_width_for_one_column(),
        ..CLARA_BW_METRICS
    };
    let screen = Screen::new(
        1,
        vec![Node::Band {
            id: NodeId(1),
            align: BandAlign::Top,
            slots: vec![
                BandSlot::fill(vec![Node::Text {
                    id: NodeId(2),
                    text: "Left".to_owned(),
                    links: Vec::new(),
                }]),
                BandSlot::fill(vec![Node::Text {
                    id: NodeId(3),
                    text: "Right".to_owned(),
                    links: Vec::new(),
                }]),
            ],
        }],
    );
    let layout = screen.layout_with(&narrow, &Chrome::default());
    let rect = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("laid out")
            .rect
    };
    assert_eq!(
        rect(2).x,
        rect(3).x,
        "a band that could not fit its columns did not fall back to stacking"
    );
    assert!(
        rect(3).y > rect(2).y,
        "a stacked band left its slots on top of each other"
    );
}

#[test]
fn a_band_slot_can_be_centred_against_a_taller_neighbour() {
    let band = |align: BandAlign| {
        let screen = Screen::new(
            1,
            vec![Node::Band {
                id: NodeId(1),
                align,
                slots: vec![
                    BandSlot::fill(vec![
                        Node::Heading {
                            id: NodeId(2),
                            text: "Tall".to_owned(),
                            level: 1,
                        },
                        Node::Heading {
                            id: NodeId(4),
                            text: "Still tall".to_owned(),
                            level: 1,
                        },
                    ]),
                    BandSlot::fill(vec![Node::Secondary {
                        id: NodeId(3),
                        text: "Short".to_owned(),
                    }]),
                ],
            }],
        );
        screen
            .layout()
            .nodes
            .iter()
            .find(|node| node.id == NodeId(3))
            .expect("laid out")
            .rect
            .y
    };
    assert!(
        band(BandAlign::Middle) > band(BandAlign::Top),
        "a centred slot was not moved down against its taller neighbour"
    );
    assert!(
        band(BandAlign::Bottom) > band(BandAlign::Middle),
        "a bottom aligned slot was not moved below a centred one"
    );
}

#[test]
fn a_natural_slot_takes_only_what_it_measures() {
    let screen = Screen::new(
        1,
        vec![Node::Band {
            id: NodeId(1),
            align: BandAlign::Top,
            slots: vec![
                BandSlot::fill(vec![Node::Text {
                    id: NodeId(2),
                    text: "A title that wants the whole line to itself".to_owned(),
                    links: Vec::new(),
                }]),
                BandSlot::natural(vec![Node::Secondary {
                    id: NodeId(3),
                    text: "32".to_owned(),
                }]),
            ],
        }],
    );
    let layout = screen.layout();
    let rect = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("laid out")
            .rect
    };
    assert!(
        rect(3).width < rect(2).width,
        "a natural slot took as much room as the fill beside it"
    );
    assert!(
        rect(3).width >= measure_text("32", FontSize::Caption).0,
        "a natural slot was clamped below what its content measures"
    );
}

/// A panel narrow enough that two columns cannot both stay readable.
fn metrics_width_for_one_column() -> i32 {
    CLARA_BW_METRICS.tenth_mm(MIN_BAND_SLOT_TENTH_MM) * 2
}

#[test]
fn a_section_is_quieter_than_the_heading_it_sits_under() {
    let screen = Screen::new(
        1,
        vec![
            Node::Heading {
                id: NodeId(1),
                text: "Moby Dick".to_owned(),
                level: 1,
            },
            Node::Section {
                id: NodeId(2),
                title: "Details".to_owned(),
                value: None,
                link: None,
            },
        ],
    );
    let layout = screen.layout();
    let node = |id: u32| {
        layout
            .nodes
            .iter()
            .find(|node| node.id == NodeId(id))
            .expect("laid out")
    };
    assert!(
        node(2).rect.height < node(1).rect.height,
        "a section was not set smaller than the screen's own heading"
    );
    assert_eq!(node(2).kind, LayoutKind::Section);
}

#[test]
fn a_section_draws_a_rule_in_the_room_its_title_leaves() {
    let paint_section = |title: &str| {
        let screen = Screen::new(
            1,
            vec![Node::Section {
                id: NodeId(1),
                title: title.to_owned(),
                value: None,
                link: None,
            }],
        );
        let rect = screen
            .layout()
            .nodes
            .iter()
            .find(|node| node.id == NodeId(1))
            .expect("laid out")
            .rect;
        let (surface, stride) = paint(&screen);
        tone_count(&surface, rect, tone::RULE, stride)
    };
    assert!(paint_section("About") > 0, "a section drew no hairline");
    assert!(
        paint_section("About") > paint_section("A rather longer section name"),
        "a longer title did not give up any of its own hairline"
    );
}

#[test]
fn a_sections_value_is_measured_before_its_title_is_clamped() {
    let screen = Screen::new(
        1,
        vec![Node::Section {
            id: NodeId(1),
            title: "A section title long enough to want the whole line".to_owned(),
            value: Some("32".to_owned()),
            link: None,
        }],
    );
    let node = screen
        .layout()
        .nodes
        .iter()
        .find(|node| node.id == NodeId(1))
        .expect("laid out")
        .clone();
    let title = node.text_lines.first().expect("a title");
    let value = node.text_lines.get(1).expect("a value");
    assert_eq!(value, "32", "the value was clamped away");
    let together =
        measure_text(title, FontSize::Caption).0 + measure_text(value, FontSize::Caption).0;
    assert!(
        together <= node.rect.width,
        "a title and its value together ran past the right margin"
    );
}
