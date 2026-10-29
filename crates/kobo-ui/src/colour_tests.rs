use super::*;

/// What the panel reflects of one channel, in the sRGB sense the contrast
/// rules are written against. Floating point is fine here: these are tests
/// of a palette that is fixed at compile time, not arithmetic on a frame.
fn linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance([red, green, blue]: [u8; 3]) -> f64 {
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

/// The WCAG contrast ratio, lighter over darker.
fn contrast(one: [u8; 3], other: [u8; 3]) -> f64 {
    let (lighter, darker) = {
        let (a, b) = (relative_luminance(one), relative_luminance(other));
        if a > b {
            (a, b)
        } else {
            (b, a)
        }
    };
    (lighter + 0.05) / (darker + 0.05)
}

const INK: [u8; 3] = [0, 0, 0];
const PAPER: [u8; 3] = [255, 255, 255];

#[test]
fn every_shade_is_one_of_the_panels_4096_colours() {
    for accent in Accent::ALL {
        let Shades { tint, strong } = accent.shades();
        for colour in [tint, strong] {
            assert!(
                colour.iter().all(|channel| channel % PANEL_LEVEL == 0),
                "{} sends {colour:?}, which a Kaleido panel would round",
                accent.name()
            );
        }
    }
}

#[test]
fn every_tint_carries_ink_and_every_strong_shade_carries_paper_at_seven_to_one() {
    // Seven to one is the strictest text contrast the accessibility rules
    // name. It is asked for here because the panel's colour filter lowers
    // every contrast below what the numbers say, and a pair that only just
    // passed on a phone would fail on paper.
    for accent in Accent::ALL {
        let Shades { tint, strong } = accent.shades();
        assert!(
            contrast(tint, INK) >= 7.0,
            "{} tint {tint:?} carries ink at only {:.1}:1",
            accent.name(),
            contrast(tint, INK)
        );
        assert!(
            contrast(strong, PAPER) >= 7.0,
            "{} strong {strong:?} carries paper at only {:.1}:1",
            accent.name(),
            contrast(strong, PAPER)
        );
    }
}

#[test]
fn the_two_shades_of_an_accent_can_be_told_apart_on_a_sixteen_grey_panel() {
    // A reader looking at the grey plane, or a greyscale photograph of the
    // panel, still sees two shades: the tint stays light and the strong shade
    // stays dark, at least four of the panel's sixteen levels apart.
    for accent in Accent::ALL {
        let Shades { tint, strong } = accent.shades();
        assert!(
            luma(tint) >= luma(strong) + 4 * PANEL_LEVEL,
            "{}: tint luma {} and strong luma {} are too close",
            accent.name(),
            luma(tint),
            luma(strong)
        );
    }
}

#[test]
fn wire_values_round_trip_and_an_unknown_value_is_no_accent() {
    for accent in Accent::ALL {
        assert_eq!(Accent::from_wire(accent.wire_value()), Some(accent));
        assert_ne!(accent.wire_value(), 0, "zero is reserved for no accent");
    }
    assert_eq!(Accent::from_wire(0), None);
    // A colour this build has no name for is drawn grey rather than refused,
    // so a newer application still opens on an older runtime.
    assert_eq!(Accent::from_wire(200), None);
    assert_eq!(Accent::from_wire(u8::MAX), None);
    let mut names: Vec<&str> = Accent::ALL.iter().map(|accent| accent.name()).collect();
    names.dedup();
    assert_eq!(names.len(), Accent::ALL.len(), "two accents share a name");
}

#[test]
fn the_grey_palette_answers_every_role_with_the_tone_it_always_had() {
    for accent in [None, Some(Accent::Orange), Some(Accent::Cobalt)] {
        let palette = Palette::new(Appearance::Grey, accent);
        assert_eq!(palette.bar(), Paint::Grey(tone::PAPER));
        assert_eq!(palette.emphasis(), Paint::Grey(tone::INK));
        assert_eq!(palette.banner_edge(), Paint::Grey(tone::INK));
        for level in [
            BannerLevel::Info,
            BannerLevel::Attention,
            BannerLevel::Success,
            BannerLevel::Error,
        ] {
            assert_eq!(palette.banner(level), Paint::Grey(tone::SURFACE));
        }
    }
    assert_eq!(Palette::grey(), Palette::new(Appearance::Grey, None));
}

#[test]
fn a_screen_without_an_accent_keeps_a_paper_bar_and_the_platforms_cobalt() {
    let palette = Palette::new(Appearance::Colour, None);
    assert_eq!(palette.bar(), Paint::Grey(tone::PAPER));
    assert_eq!(
        palette.emphasis(),
        Paint::Colour(Accent::Cobalt.shades().strong)
    );
    let orange = Palette::new(Appearance::Colour, Some(Accent::Orange));
    assert_eq!(orange.bar(), Paint::Colour(Accent::Orange.shades().tint));
    assert_eq!(
        orange.emphasis(),
        Paint::Colour(Accent::Orange.shades().strong)
    );
}

#[test]
fn banners_take_the_platforms_colours_whatever_the_applications_accent_is() {
    for accent in [None, Some(Accent::Orange), Some(Accent::Red)] {
        let palette = Palette::new(Appearance::Colour, accent);
        assert_eq!(
            palette.banner(BannerLevel::Info),
            Paint::Grey(tone::SURFACE)
        );
        assert_eq!(
            palette.banner(BannerLevel::Success),
            Paint::Colour(Accent::Green.shades().tint)
        );
        assert_eq!(
            palette.banner(BannerLevel::Error),
            Paint::Colour(Accent::Red.shades().tint)
        );
        assert_eq!(
            palette.banner(BannerLevel::Attention),
            Paint::Colour(Accent::Amber.shades().tint)
        );
        assert_eq!(
            palette.banner_edge(),
            Paint::Colour(Accent::Amber.shades().strong)
        );
    }
}

#[test]
fn a_colour_fill_keeps_the_grey_plane_at_its_luminance_and_a_grey_fill_takes_the_colour_back() {
    let mut surface = Surface::new(4, 4);
    let rect = Rect {
        x: 1,
        y: 1,
        width: 2,
        height: 2,
    };
    let strong = Accent::Teal.shades().strong;
    surface.fill_rect_paint(rect, Paint::Colour(strong));
    assert_eq!(surface.rgb_at(5), Some(strong));
    assert_eq!(surface.pixels[5], luma(strong));
    assert_eq!(surface.rgb_at(0), Some([tone::PAPER; 3]));
    assert!(surface.has_colour());
    surface.fill_rect_paint(rect, Paint::Grey(tone::INK));
    assert_eq!(surface.rgb_at(5), Some([tone::INK; 3]));
    assert!(!surface.has_colour());
    // A grey paint on a surface with no colour plane never creates one.
    let mut grey = Surface::new(4, 4);
    grey.fill_rect_paint(rect, Paint::Grey(tone::SURFACE));
    assert!(grey.chroma.is_none());
}

fn chip(action: u32, label: &str, selected: bool) -> Chip {
    Chip {
        action: ActionId(action),
        label: label.to_owned(),
        selected,
    }
}

fn checked_row(state: RowState) -> Node {
    let mut row = Row::new(ActionId(40), "Keep", "", RowLead::Icon(Glyph::Check));
    row.state = state;
    Node::Rows {
        id: NodeId(41),
        rows: vec![row],
    }
}

fn options(selected: Option<u16>) -> Node {
    Node::Options {
        id: NodeId(50),
        prompt: "Which".to_owned(),
        options: vec![
            OptionRow {
                action: ActionId(51),
                label: "One".to_owned(),
                summary: String::new(),
            },
            OptionRow {
                action: ActionId(52),
                label: "Two".to_owned(),
                summary: String::new(),
            },
        ],
        selected,
        page: 0,
        previous: ActionId(53),
        next: ActionId(54),
    }
}

fn toggle(on: bool) -> Node {
    Node::Toggle {
        id: NodeId(60),
        action: ActionId(61),
        label: "Sleep".to_owned(),
        summary: String::new(),
        on,
        state: ControlState::Enabled,
    }
}

fn button(emphasis: Emphasis) -> Node {
    Node::Button {
        id: NodeId(70),
        action: ActionId(71),
        label: "Save".to_owned(),
        state: ControlState::Enabled,
        emphasis,
    }
}

fn banner(level: BannerLevel) -> Node {
    Node::Banner {
        id: NodeId(80),
        level,
        text: "Saved to this reader".to_owned(),
        action: None,
    }
}

/// Everything that can take colour, on one screen, under an accented bar.
fn kitchen_sink(accent: Option<Accent>) -> Screen {
    Screen::new(
        1,
        vec![
            Node::Heading {
                id: NodeId(2),
                text: "Heading".to_owned(),
                level: 1,
            },
            Node::Text {
                id: NodeId(3),
                text: "Body text under the heading.".to_owned(),
                links: Vec::new(),
            },
            button(Emphasis::Primary),
            Node::Chips {
                id: NodeId(10),
                chips: vec![chip(11, "All", true), chip(12, "Unread", false)],
            },
            Node::Tabs {
                id: NodeId(20),
                tabs: vec![chip(21, "Top", false), chip(22, "New", false)],
                selected: 0,
            },
            Node::Progress {
                id: NodeId(30),
                value: Percent::new(40),
            },
            toggle(true),
            checked_row(RowState::Checked),
            Node::Steps {
                id: NodeId(90),
                current: 2,
                total: 4,
                labels: Vec::new(),
            },
            banner(BannerLevel::Success),
        ],
    )
    .with_top_bar(TopBar::new(NodeId(100), "Hacker News").action(ActionId(101), "Refresh"))
    .with_nav_bar(NavBar::new(
        NodeId(110),
        vec![
            BarAction::new(ActionId(111), "Top"),
            BarAction::new(ActionId(112), "New"),
            BarAction::new(ActionId(113), "Saved"),
        ],
        Some(0),
    ))
    .with_accent(accent)
}

fn paint(screen: &Screen, appearance: Appearance) -> Surface {
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true).with_appearance(appearance);
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(screen, &metrics, &chrome, &mut surface, None);
    surface
}

fn rect_of(screen: &Screen, wanted: fn(&LayoutKind) -> bool) -> Rect {
    screen
        .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .nodes
        .iter()
        .find(|node| wanted(&node.kind))
        .map(|node| node.rect)
        .expect("the screen lays the node out")
}

fn count_colour(surface: &Surface, colour: [u8; 3]) -> usize {
    (0..surface.pixels.len())
        .filter(|index| surface.rgb_at(*index) == Some(colour))
        .count()
}

#[test]
fn a_grey_panel_never_grows_a_colour_plane_whatever_the_screen_asks_for() {
    let accented = paint(&kitchen_sink(Some(Accent::Orange)), Appearance::Grey);
    let plain = paint(&kitchen_sink(None), Appearance::Grey);
    assert!(accented.chroma.is_none(), "a grey panel allocated colour");
    assert_eq!(
        accented.pixels, plain.pixels,
        "an accent changed what a grey panel shows"
    );
}

#[test]
fn colour_moves_nothing() {
    // Layout never sees the appearance or the accent, so an application
    // that paginated for itself on a Clara BW has paginated for a Clara
    // Colour too. Held as a test rather than left to the type system because
    // the chrome, which does carry the appearance, is a layout input.
    for accent in [None, Some(Accent::Purple)] {
        let screen = kitchen_sink(accent);
        let grey = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let colour = screen.layout_with(
            &CLARA_BW_METRICS,
            &Chrome::measuring(true).with_appearance(Appearance::Colour),
        );
        assert_eq!(grey.nodes, colour.nodes);
    }
    let plain = kitchen_sink(None).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    let accented =
        kitchen_sink(Some(Accent::Red)).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    assert_eq!(plain.nodes, accented.nodes);
}

#[test]
fn the_grey_plane_of_a_colour_frame_is_the_luminance_of_every_pixel() {
    let surface = paint(&kitchen_sink(Some(Accent::Green)), Appearance::Colour);
    let chroma = surface.chroma.as_ref().expect("a colour frame");
    for (index, grey) in surface.pixels.iter().enumerate() {
        let colour = [
            chroma[index * 3],
            chroma[index * 3 + 1],
            chroma[index * 3 + 2],
        ];
        // Exact on every fill. Along a glyph edge the two planes are blended
        // separately and each rounds for itself, so they may differ by one
        // of two hundred and fifty-five, which is a sixteenth of one of the
        // panel's sixteen levels.
        assert!(
            grey.abs_diff(luma(colour)) <= 1,
            "pixel {index} is {colour:?} in colour and {grey} in grey"
        );
    }
}

#[test]
fn an_accented_bar_is_the_tint_with_its_title_and_controls_still_in_ink() {
    let screen = kitchen_sink(Some(Accent::Orange));
    let surface = paint(&screen, Appearance::Colour);
    let Shades { tint, .. } = Accent::Orange.shades();
    let bar = rect_of(&screen, |kind| *kind == LayoutKind::TopBar);
    let at = |x: i32, y: i32| surface.rgb_at(y as usize * surface.width + x as usize);
    // The corners of the bar carry nothing but the field.
    assert_eq!(at(bar.x + 1, bar.y + 1), Some(tint));
    assert_eq!(
        at(bar.x + bar.width - 2, bar.y + bar.height - 2),
        Some(tint)
    );
    let title = rect_of(&screen, |kind| *kind == LayoutKind::TopBarTitle);
    let mut ink = 0;
    for y in title.y..title.y + title.height {
        for x in title.x..title.x + title.width {
            if at(x, y) == Some([0, 0, 0]) {
                ink += 1;
            }
        }
    }
    assert!(ink > 0, "the title is not drawn in ink on the tint");
    // The band above the bar stays paper, so the tint is a bar and not a
    // header across the whole top of the panel.
    let band = rect_of(&screen, |kind| *kind == LayoutKind::StatusBand);
    assert_eq!(at(band.x + 1, band.y + 1), Some([tone::PAPER; 3]));
    // And the bottom bar stays paper too.
    let nav = rect_of(&screen, |kind| *kind == LayoutKind::NavBar);
    assert_eq!(
        at(nav.x + 1, nav.y + nav.height - 2),
        Some([tone::PAPER; 3])
    );
}

#[test]
fn a_screen_with_no_accent_is_drawn_in_cobalt_with_a_paper_bar() {
    let screen = kitchen_sink(None);
    let surface = paint(&screen, Appearance::Colour);
    let bar = rect_of(&screen, |kind| *kind == LayoutKind::TopBar);
    assert_eq!(
        surface.rgb_at((bar.y + 1) as usize * surface.width + (bar.x + 1) as usize),
        Some([tone::PAPER; 3])
    );
    assert!(count_colour(&surface, Accent::Cobalt.shades().strong) > 0);
    assert_eq!(count_colour(&surface, Accent::Cobalt.shades().tint), 0);
}

#[test]
fn the_primary_control_wears_the_strong_shade_with_a_paper_label() {
    let screen = kitchen_sink(Some(Accent::Red));
    let surface = paint(&screen, Appearance::Colour);
    let Shades { strong, .. } = Accent::Red.shades();
    let button = rect_of(&screen, |kind| {
        matches!(
            kind,
            LayoutKind::Button(_, ControlState::Enabled, Emphasis::Primary)
        )
    });
    let at = |x: i32, y: i32| surface.rgb_at(y as usize * surface.width + x as usize);
    // Inside the corner radius, in from the edge, away from the label.
    assert_eq!(
        at(button.x + button.width / 8, button.y + button.height / 2),
        Some(strong)
    );
    let mut paper = 0;
    for y in button.y..button.y + button.height {
        for x in button.x..button.x + button.width {
            if at(x, y) == Some([tone::PAPER; 3]) {
                paper += 1;
            }
        }
    }
    assert!(paper > 0, "the label is not reversed out of the fill");
}

#[test]
fn a_control_takes_colour_only_when_it_is_on() {
    let strong = Accent::Teal.shades().strong;
    let cases: [(Node, Node); 5] = [
        (toggle(true), toggle(false)),
        (
            Node::Chips {
                id: NodeId(10),
                chips: vec![chip(11, "All", true)],
            },
            Node::Chips {
                id: NodeId(10),
                chips: vec![chip(11, "All", false)],
            },
        ),
        (
            Node::Progress {
                id: NodeId(30),
                value: Percent::new(60),
            },
            Node::Progress {
                id: NodeId(30),
                value: Percent::new(0),
            },
        ),
        (
            checked_row(RowState::Checked),
            checked_row(RowState::Unchecked),
        ),
        (options(Some(1)), options(None)),
    ];
    for (on, off) in cases {
        let lit = paint(
            &Screen::new(1, vec![on.clone()]).with_accent(Some(Accent::Teal)),
            Appearance::Colour,
        );
        assert!(
            count_colour(&lit, strong) > 0,
            "{on:?} carries no colour when it is on"
        );
        let dark = paint(
            &Screen::new(1, vec![off.clone()]).with_accent(Some(Accent::Teal)),
            Appearance::Colour,
        );
        assert!(
            dark.chroma.is_none(),
            "{off:?} carries colour although nothing on it is on"
        );
    }
}

#[test]
fn a_step_indicator_always_marks_the_step_the_reader_is_on() {
    // Unlike a switch there is no off: one step is always current, so the
    // ring around it carries the accent even before any step is done, and
    // the rings for the steps still to come stay in the rule's grey.
    let strong = Accent::Teal.shades().strong;
    for current in 0..4 {
        let screen = Screen::new(
            1,
            vec![Node::Steps {
                id: NodeId(90),
                current,
                total: 4,
                labels: Vec::new(),
            }],
        )
        .with_accent(Some(Accent::Teal));
        assert!(count_colour(&paint(&screen, Appearance::Colour), strong) > 0);
    }
}

#[test]
fn a_tab_and_a_destination_mark_where_the_reader_is_in_the_accent() {
    let strong = Accent::Blue.shades().strong;
    let tabs = Screen::new(
        1,
        vec![Node::Tabs {
            id: NodeId(20),
            tabs: vec![chip(21, "Top", false), chip(22, "New", false)],
            selected: 1,
        }],
    )
    .with_accent(Some(Accent::Blue));
    assert!(count_colour(&paint(&tabs, Appearance::Colour), strong) > 0);
    let nav = Screen::new(1, Vec::new())
        .with_nav_bar(NavBar::new(
            NodeId(110),
            vec![
                BarAction::new(ActionId(111), "Top"),
                BarAction::new(ActionId(112), "New"),
            ],
            Some(1),
        ))
        .with_accent(Some(Accent::Blue));
    assert!(count_colour(&paint(&nav, Appearance::Colour), strong) > 0);
    // A bar of verbs marks nothing, so it carries no colour either.
    let mut verbs = NavBar::new(
        NodeId(110),
        vec![
            BarAction::new(ActionId(111), "Previous"),
            BarAction::new(ActionId(112), "Next"),
        ],
        None,
    );
    verbs.style = BarStyle::Actions;
    let actions = Screen::new(1, Vec::new())
        .with_nav_bar(verbs)
        .with_accent(Some(Accent::Blue));
    assert!(paint(&actions, Appearance::Colour).chroma.is_none());
}

#[test]
fn a_banner_keeps_its_mark_and_outline_so_colour_is_never_the_only_signal() {
    for level in [
        BannerLevel::Success,
        BannerLevel::Error,
        BannerLevel::Attention,
    ] {
        let screen = Screen::new(1, vec![banner(level)]);
        let grey = paint(&screen, Appearance::Grey);
        let colour = paint(&screen, Appearance::Colour);
        let field = Palette::new(Appearance::Colour, None).banner(level);
        let Paint::Colour(tint) = field else {
            panic!("{level:?} has no colour field");
        };
        assert!(
            count_colour(&colour, tint) > 0,
            "{level:?} field is not tinted"
        );
        // Every pixel that is ink on the grey panel is still a mark on the
        // colour one: the glyph, the rule and the words stay ink, and the
        // attention banner's edge becomes the strong shade of its own field.
        // The colour only ever replaces the quiet surface behind them.
        let chroma = colour.chroma.as_ref().expect("a tinted banner");
        let edge = Accent::Amber.shades().strong;
        for (index, grey_pixel) in grey.pixels.iter().enumerate() {
            if *grey_pixel == tone::INK {
                let pixel = &chroma[index * 3..index * 3 + 3];
                assert!(
                    pixel == [tone::INK; 3] || (level == BannerLevel::Attention && pixel == edge),
                    "{level:?} lost an ink pixel at {index}: {pixel:?}"
                );
            }
        }
    }
    let neutral = paint(
        &Screen::new(1, vec![banner(BannerLevel::Info)]),
        Appearance::Colour,
    );
    assert!(neutral.chroma.is_none(), "a neutral banner took a colour");
}

#[test]
fn text_is_never_painted_in_a_colour_of_its_own() {
    // The only colours on a frame are the shades the palette names and the
    // tones blended with them along glyph edges. A text run in a third
    // colour would show up as a channel ordering the palette never produces.
    let screen = kitchen_sink(Some(Accent::Amber));
    let surface = paint(&screen, Appearance::Colour);
    let chroma = surface.chroma.as_ref().expect("a colour frame");
    let allowed: Vec<[u8; 3]> = [
        Accent::Amber.shades().tint,
        Accent::Amber.shades().strong,
        Accent::Green.shades().tint,
    ]
    .into();
    for pixel in chroma.chunks_exact(3) {
        let colour = [pixel[0], pixel[1], pixel[2]];
        if colour[0] == colour[1] && colour[1] == colour[2] {
            continue;
        }
        // A blend of an allowed colour towards a grey keeps its channel
        // order, so a colour that is none of them and no blend of them is
        // one the painter has no business making.
        let is_blend = allowed.iter().any(|shade| {
            let order = |c: [u8; 3]| [c[0].cmp(&c[1]), c[1].cmp(&c[2]), c[0].cmp(&c[2])];
            order(*shade)
                .iter()
                .zip(order(colour))
                .all(|(a, b)| *a == b || b == std::cmp::Ordering::Equal)
        });
        assert!(
            is_blend,
            "{colour:?} is not a palette colour or a blend of one"
        );
    }
}

fn status(level: u8, charging: bool, signal: Signal) -> Status {
    Status::standard("09:41", Some(Percent::new(level)), charging, signal, false)
}

fn painted_with_status(status: Status, appearance: Appearance) -> Surface {
    let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Status"));
    let chrome = Chrome::with_back(false)
        .with_status(status)
        .with_appearance(appearance);
    let mut surface = Surface::new(
        CLARA_BW_METRICS.width as usize,
        CLARA_BW_METRICS.height as usize,
    );
    render_with(&screen, &CLARA_BW_METRICS, &chrome, &mut surface, None);
    surface
}

#[test]
fn the_battery_mark_is_red_when_low_green_when_charging_and_ink_otherwise() {
    let red = Accent::Red.shades().strong;
    let green = Accent::Green.shades().strong;
    let low = painted_with_status(
        status(LOW_BATTERY_PERCENT, false, Signal::Strong),
        Appearance::Colour,
    );
    assert!(count_colour(&low, red) > 0, "a low battery is not red");
    assert_eq!(count_colour(&low, green), 0);
    let charging = painted_with_status(
        status(LOW_BATTERY_PERCENT, true, Signal::Strong),
        Appearance::Colour,
    );
    assert!(
        count_colour(&charging, green) > 0,
        "a charging battery is not green"
    );
    assert_eq!(count_colour(&charging, red), 0, "charging outranks low");
    let fine = painted_with_status(status(72, false, Signal::Strong), Appearance::Colour);
    assert!(fine.chroma.is_none(), "an ordinary battery took a colour");
    // The number beside the mark stays ink: it is text.
    let band = low
        .pixels
        .iter()
        .zip(low.chroma.as_ref().expect("a colour band"))
        .count();
    assert!(band > 0);
}

#[test]
fn the_radio_mark_is_amber_only_when_the_radio_is_on_and_nothing_gets_through() {
    let amber = Accent::Amber.shades().strong;
    let stuck = painted_with_status(status(72, false, Signal::Disconnected), Appearance::Colour);
    assert!(
        count_colour(&stuck, amber) > 0,
        "a disconnected radio is not amber"
    );
    for signal in [Signal::Off, Signal::Weak, Signal::Fair, Signal::Strong] {
        let surface = painted_with_status(status(72, false, signal), Appearance::Colour);
        assert!(surface.chroma.is_none(), "{signal:?} took a colour");
    }
}

#[test]
fn the_status_band_stays_ink_on_a_grey_panel_whatever_the_battery_does() {
    for (level, charging, signal) in [
        (5, false, Signal::Disconnected),
        (10, true, Signal::Strong),
        (100, false, Signal::Off),
    ] {
        let surface = painted_with_status(status(level, charging, signal), Appearance::Grey);
        assert!(surface.chroma.is_none());
    }
}

fn square_grid(columns: u8, selected: Option<usize>) -> Node {
    // One man on the board, which is what tells the layout this square grid
    // is a draughts board with dark squares rather than a plain grid.
    let cells = (0..usize::from(columns) * usize::from(columns))
        .map(|index| {
            let mut cell = Cell::new(ActionId(100 + index as u32), "");
            if index == 0 {
                cell.glyph = Some(Glyph::WhiteDraughtsMan);
            }
            cell.with_selected(selected == Some(index))
        })
        .collect();
    Node::Grid {
        id: NodeId(200),
        columns,
        square: true,
        cells,
    }
}

#[test]
fn a_board_keeps_brown_squares_whatever_the_applications_accent_and_marks_the_square_in_hand() {
    let brown = Accent::Brown.shades().tint;
    for accent in [None, Some(Accent::Orange)] {
        let board = Screen::new(1, vec![square_grid(8, None)]).with_accent(accent);
        let painted = paint(&board, Appearance::Colour);
        assert!(
            count_colour(&painted, brown) > 0,
            "no dark squares in brown"
        );
        assert_eq!(
            count_colour(&painted, Accent::Orange.shades().tint),
            0,
            "the board took the application's colour"
        );
    }
    // The selected light square takes the accent's tint, the only place the
    // accent reaches a board.
    let selected = Screen::new(1, vec![square_grid(8, Some(2))]).with_accent(Some(Accent::Teal));
    let painted = paint(&selected, Appearance::Colour);
    assert!(count_colour(&painted, Accent::Teal.shades().tint) > 0);
    // Grey panels draw the surface grey they always did.
    assert!(paint(&selected, Appearance::Grey).chroma.is_none());
}

#[test]
fn a_plain_grids_selected_cells_keep_their_grey_because_selection_there_is_the_apps_to_define() {
    // Minesweeper marks every covered square as selected. Fifteen tinted
    // squares out of sixteen would be a field of colour, not a mark, so a
    // grid without a board's light and dark squares keeps the quiet grey.
    let cells = (0..16)
        .map(|index| Cell::new(ActionId(600 + index), "").with_selected(index != 0))
        .collect();
    let covered = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(601),
            columns: 4,
            square: true,
            cells,
        }],
    )
    .with_accent(Some(Accent::Teal));
    assert!(paint(&covered, Appearance::Colour).chroma.is_none());
}

#[test]
fn a_crossword_lights_the_word_being_solved_in_the_accents_tint() {
    let mut cells: Vec<Cell> = (0..9)
        .map(|index| Cell::new(ActionId(300 + index), "A"))
        .collect();
    for cell in &mut cells {
        cell.corner = Some(1);
    }
    cells[4].selected = true;
    let crossword = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(301),
            columns: 3,
            square: true,
            cells,
        }],
    );
    let plain = paint(&crossword, Appearance::Colour);
    assert!(
        count_colour(&plain, Accent::Cobalt.shades().tint) > 0,
        "no selection field"
    );
    assert!(paint(&crossword, Appearance::Grey).chroma.is_none());
}

#[test]
fn a_selected_pencil_cell_is_the_boards_one_filled_control() {
    let board = PencilBoard {
        columns: 3,
        rows: 3,
        cell_tenth_mm: 90,
        marks: (0..9)
            .map(|index| PencilMark {
                column: index % 3,
                row: index / 3,
                kind: PencilMarkKind::Digit {
                    value: 1 + index,
                    given: false,
                },
                action: Some(ActionId(400 + u32::from(index))),
                selected: index == 4,
                peer: index == 3 || index == 5,
            })
            .collect(),
        edges: Vec::new(),
    };
    let screen = Screen::new(
        1,
        vec![Node::PencilBoard {
            id: NodeId(401),
            board,
        }],
    )
    .with_accent(Some(Accent::Purple));
    let painted = paint(&screen, Appearance::Colour);
    assert!(count_colour(&painted, Accent::Purple.shades().strong) > 0);
    assert_eq!(
        count_colour(&painted, Accent::Purple.shades().tint),
        0,
        "peers took a colour; they stay on the quiet grey"
    );
    assert!(paint(&screen, Appearance::Grey).chroma.is_none());
}

#[test]
fn a_refused_field_is_tinted_red_under_its_note_and_an_accepted_one_is_not() {
    let field = |error: Option<&str>| {
        Screen::new(
            1,
            vec![Node::Field {
                id: NodeId(500),
                action: ActionId(501),
                value: "nonsense".to_owned(),
                placeholder: String::new(),
                clear: None,
                label: Some("Address".to_owned()),
                hint: None,
                error: error.map(str::to_owned),
                kind: FieldKind::Text,
            }],
        )
    };
    let refused = paint(&field(Some("Not an address")), Appearance::Colour);
    assert!(
        count_colour(&refused, Accent::Red.shades().tint) > 0,
        "no red field"
    );
    let accepted = paint(&field(None), Appearance::Colour);
    assert!(accepted.chroma.is_none());
    assert!(paint(&field(Some("Not an address")), Appearance::Grey)
        .chroma
        .is_none());
}
