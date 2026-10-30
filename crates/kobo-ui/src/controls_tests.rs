use super::tests::PANELS;
use super::*;

fn screen(nodes: Vec<Node>) -> Screen {
    Screen::new(1, nodes).with_top_bar(TopBar::new(NodeId(90), "Controls"))
}

fn kinds(layout: &Layout, wanted: fn(&LayoutKind) -> bool) -> Vec<&LayoutNode> {
    layout
        .nodes
        .iter()
        .filter(|node| wanted(&node.kind))
        .collect()
}

fn centre(rect: Rect) -> (i32, i32) {
    (rect.x + rect.width / 2, rect.y + rect.height / 2)
}

fn toggle(label: &str, on: bool, state: ControlState) -> Node {
    Node::Toggle {
        id: NodeId(1),
        action: ActionId(7),
        label: label.to_owned(),
        summary: "Turns the panel off after five minutes untouched.".to_owned(),
        on,
        state,
    }
}

fn painted(screen: &Screen, metrics: &DisplayMetrics) -> Surface {
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(screen, metrics, &Chrome::default(), &mut surface, None);
    surface
}

fn changed_inside(before: &Surface, after: &Surface, bounds: Rect) -> usize {
    let mut changed = 0;
    for (index, (old, new)) in before.pixels.iter().zip(&after.pixels).enumerate() {
        if old == new {
            continue;
        }
        changed += 1;
        let x = i32::try_from(index % after.width).unwrap();
        let y = i32::try_from(index / after.width).unwrap();
        assert!(bounds.contains(x, y), "{x},{y} changed outside {bounds:?}");
    }
    changed
}

#[test]
fn a_toggle_is_one_row_high_whatever_its_label_says() {
    for (name, metrics) in PANELS {
        let short = screen(vec![toggle("Sleep", true, ControlState::Enabled)]).layout_for(&metrics);
        let long = screen(vec![toggle(
            "Put the reader to sleep when the cover closes and wake it when it opens",
            true,
            ControlState::Enabled,
        )])
        .layout_for(&metrics);
        let row =
            |layout: &Layout| kinds(layout, |kind| matches!(kind, LayoutKind::Toggle(..)))[0].rect;
        assert_eq!(row(&short), row(&long), "{name}");
        let expected = with_text_scale(metrics.text_scale, || one_line_row_height(&metrics, true));
        assert_eq!(row(&short).height, expected, "{name}");
        let label = kinds(&long, |kind| matches!(kind, LayoutKind::ToggleLabel(_)))[0];
        assert_eq!(label.text_lines.len(), 1, "{name}: {:?}", label.text_lines);
    }
}

#[test]
fn a_tap_anywhere_on_a_toggle_reports_it_and_a_disabled_one_reports_nothing() {
    let enabled = screen(vec![toggle("Sleep", false, ControlState::Enabled)])
        .with_page_turns(ActionId(50), ActionId(51));
    let layout = enabled.layout();
    let row = kinds(&layout, |kind| matches!(kind, LayoutKind::Toggle(..)))[0].rect;
    let switch = kinds(&layout, |kind| matches!(kind, LayoutKind::ToggleSwitch(..)))[0].rect;
    assert_eq!(layout.hit_test(row.x + 4, row.y + 4), Some(ActionId(7)));
    let (x, y) = centre(switch);
    assert_eq!(layout.hit_test(x, y), Some(ActionId(7)));

    let disabled = screen(vec![toggle("Sleep", false, ControlState::Disabled)])
        .with_page_turns(ActionId(50), ActionId(51));
    let layout = disabled.layout();
    let (x, y) = centre(row);
    // Not the toggle, and not the page turn underneath it either.
    assert_eq!(layout.hit_test(x, y), None);
}

#[test]
fn turning_a_toggle_on_repaints_only_its_switch() {
    let metrics = CLARA_BW_METRICS;
    let off = screen(vec![toggle("Sleep", false, ControlState::Enabled)]);
    let on = screen(vec![toggle("Sleep", true, ControlState::Enabled)]);
    let switch = kinds(&on.layout_for(&metrics), |kind| {
        matches!(kind, LayoutKind::ToggleSwitch(..))
    })[0]
        .rect;
    let changed = changed_inside(&painted(&off, &metrics), &painted(&on, &metrics), switch);
    assert!(changed > 0);
}

#[test]
fn a_switch_is_drawn_in_ink_and_paper_alone() {
    let metrics = CLARA_BW_METRICS;
    for on in [false, true] {
        let shown = screen(vec![toggle("Sleep", on, ControlState::Enabled)]);
        let switch = kinds(&shown.layout_for(&metrics), |kind| {
            matches!(kind, LayoutKind::ToggleSwitch(..))
        })[0]
            .rect;
        let surface = painted(&shown, &metrics);
        for y in switch.y..switch.y + switch.height {
            for x in switch.x..switch.x + switch.width {
                let pixel = surface.pixels[y as usize * surface.width + x as usize];
                assert!(
                    pixel == tone::INK || pixel == tone::PAPER,
                    "on {on}: grey {pixel} at {x},{y}"
                );
            }
        }
    }
}

fn rows_with(state: RowState, badge: Option<u32>, button: bool) -> Screen {
    let mut row = Row::new(
        ActionId(3),
        "Field guide to the birds",
        "Updated",
        Glyph::Book,
    );
    row.state = state;
    row.badge = badge;
    if button {
        row = row.with_button(ActionId(4), "Update");
    }
    screen(vec![Node::Rows {
        id: NodeId(1),
        rows: vec![row],
    }])
}

#[test]
fn a_ticked_row_is_exactly_as_tall_and_as_wide_as_an_open_one() {
    for (name, metrics) in PANELS {
        let open = rows_with(RowState::Open, None, false).layout_for(&metrics);
        let target =
            |layout: &Layout| kinds(layout, |kind| matches!(kind, LayoutKind::Row(_)))[0].rect;
        let title =
            |layout: &Layout| kinds(layout, |kind| matches!(kind, LayoutKind::RowTitle))[0].rect;
        for state in [RowState::Checked, RowState::Unchecked, RowState::Mixed] {
            let ticked = rows_with(state, None, false).layout_for(&metrics);
            assert_eq!(target(&open), target(&ticked), "{name} {state:?}");
            assert_eq!(title(&open), title(&ticked), "{name} {state:?}");
            assert!(kinds(&ticked, |kind| matches!(kind, LayoutKind::RowLead(_))).is_empty());
            let check = kinds(&ticked, |kind| matches!(kind, LayoutKind::RowCheck(_)));
            assert_eq!(check.len(), 1, "{name} {state:?}");
        }
    }
}

#[test]
fn the_three_check_states_draw_three_different_boxes() {
    let metrics = CLARA_BW_METRICS;
    let boxes = [RowState::Checked, RowState::Unchecked, RowState::Mixed]
        .map(|state| painted(&rows_with(state, None, false), &metrics).pixels);
    assert_ne!(boxes[0], boxes[1]);
    assert_ne!(boxes[1], boxes[2]);
    assert_ne!(boxes[0], boxes[2]);
}

#[test]
fn a_badge_keeps_one_width_whether_it_counts_nine_or_a_thousand() {
    for (name, metrics) in PANELS {
        let measure = |count| {
            let layout = rows_with(RowState::Open, Some(count), false).layout_for(&metrics);
            let badge = kinds(&layout, |kind| matches!(kind, LayoutKind::RowBadge))[0].clone();
            let title = kinds(&layout, |kind| matches!(kind, LayoutKind::RowTitle))[0].clone();
            (badge, title)
        };
        let (nine, title_nine) = measure(9);
        let (ten, title_ten) = measure(10);
        let (many, title_many) = measure(12_000);
        assert_eq!(nine.rect, ten.rect, "{name}");
        assert_eq!(nine.rect, many.rect, "{name}");
        assert_eq!(title_nine, title_ten, "{name}");
        assert_eq!(title_nine, title_many, "{name}");
        assert_eq!(many.text_lines, vec!["999+".to_owned()]);
    }
}

#[test]
fn a_row_button_answers_before_the_row_and_the_rest_of_the_row_still_opens_it() {
    let layout = rows_with(RowState::Open, Some(3), true).layout();
    let button = kinds(&layout, |kind| matches!(kind, LayoutKind::RowButton(_)))[0].rect;
    let row = kinds(&layout, |kind| matches!(kind, LayoutKind::Row(_)))[0].rect;
    let (x, y) = centre(button);
    assert_eq!(layout.hit_test(x, y), Some(ActionId(4)));
    assert_eq!(
        layout.hit_test(row.x + 4, row.y + row.height / 2),
        Some(ActionId(3))
    );
    assert_eq!(button.height, row.height);
    let open = rows_with(RowState::Open, None, false).layout();
    assert_eq!(
        kinds(&open, |kind| matches!(kind, LayoutKind::Row(_)))[0]
            .rect
            .height,
        row.height
    );
}

fn selection(count: u32, verbs: usize) -> Screen {
    screen(vec![Node::SelectionBar {
        id: NodeId(1),
        count,
        actions: (0..verbs)
            .map(|index| BarAction::new(ActionId(20 + index as u32), format!("Verb {index}")))
            .collect(),
    }])
}

#[test]
fn the_selection_bar_is_one_bar_high_and_draws_at_most_three_verbs() {
    for (name, metrics) in PANELS {
        let layout = selection(4, 5).layout_for(&metrics);
        let bar = kinds(&layout, |kind| matches!(kind, LayoutKind::SelectionBar))[0];
        let expected = with_text_scale(metrics.text_scale, || selection_bar_height(&metrics));
        assert_eq!(bar.rect.height, expected, "{name}");
        assert_eq!(bar.text_lines, vec!["4 selected".to_owned()]);
        let verbs = kinds(&layout, |kind| {
            matches!(kind, LayoutKind::SelectionAction(_))
        });
        assert_eq!(verbs.len(), MAX_SELECTION_ACTIONS, "{name}");
        for verb in &verbs {
            assert_eq!(verb.rect.height, bar.rect.height);
            let (x, y) = centre(verb.rect);
            assert_eq!(layout.hit_test(x, y), verb.kind.acts_on(), "{name}");
        }
    }
    let issues = selection(4, 5).validate(&CLARA_BW_METRICS);
    assert!(issues.iter().any(|issue| matches!(
        issue.kind,
        LayoutIssueKind::CollectionTruncated {
            collection: "selection actions",
            ..
        }
    )));
}

fn options(count: usize, selected: Option<u16>, page: u16) -> Screen {
    screen(vec![Node::Options {
        id: NodeId(1),
        prompt: "Time zone".to_owned(),
        options: (0..count)
            .map(|index| {
                OptionRow::new(
                    ActionId(100 + index as u32),
                    format!("Zone {index}"),
                    format!("UTC+{index}"),
                )
            })
            .collect(),
        selected,
        page,
        previous: ActionId(60),
        next: ActionId(61),
    }])
}

#[test]
fn an_option_list_shows_one_page_and_turns_with_its_own_chevrons() {
    for (name, metrics) in PANELS {
        let per_page = options_per_page(&metrics, true);
        let count = per_page * 2 + 1;
        let pages = options_pages(count, &metrics, true);
        assert_eq!(pages, 3, "{name}");
        let first = options(count, None, 0).layout_for(&metrics);
        let rows = kinds(&first, |kind| matches!(kind, LayoutKind::OptionRow(..)));
        assert_eq!(rows.len(), per_page, "{name}");
        assert!(kinds(&first, |kind| matches!(kind, LayoutKind::PagePrevious(_))).is_empty());
        let next = kinds(&first, |kind| matches!(kind, LayoutKind::PageNext(_)))[0].rect;
        let (x, y) = centre(next);
        assert_eq!(first.hit_test(x, y), Some(ActionId(61)), "{name}");

        let last = options(count, None, 2).layout_for(&metrics);
        let rows = kinds(&last, |kind| matches!(kind, LayoutKind::OptionRow(..)));
        assert_eq!(rows.len(), 1, "{name}");
        assert!(kinds(&last, |kind| matches!(kind, LayoutKind::PageNext(_))).is_empty());
        let previous = kinds(&last, |kind| matches!(kind, LayoutKind::PagePrevious(_)))[0].rect;
        let (x, y) = centre(previous);
        assert_eq!(last.hit_test(x, y), Some(ActionId(60)), "{name}");

        // The pager stays where the finger left it, even on a short page.
        let band = |layout: &Layout| {
            kinds(layout, |kind| matches!(kind, LayoutKind::PagePosition))[0].clone()
        };
        assert_eq!(band(&first).rect, band(&last).rect, "{name}");
        assert_eq!(band(&last).text_lines, vec!["3 of 3".to_owned()]);
    }
}

#[test]
fn an_option_list_marks_the_answer_on_whichever_page_it_is() {
    let metrics = CLARA_BW_METRICS;
    let per_page = options_per_page(&metrics, true);
    let selected = u16::try_from(per_page + 1).unwrap();
    let page = options(per_page * 2, Some(selected), 1).layout_for(&metrics);
    let chosen = kinds(&page, |kind| matches!(kind, LayoutKind::OptionRow(_, true)));
    assert_eq!(chosen.len(), 1);
    assert_eq!(
        chosen[0].kind.acts_on(),
        Some(ActionId(100 + u32::from(selected)))
    );
    let first = options(per_page * 2, Some(selected), 0).layout_for(&metrics);
    assert!(kinds(&first, |kind| matches!(
        kind,
        LayoutKind::OptionRow(_, true)
    ))
    .is_empty());
}

#[test]
fn a_page_past_the_end_of_an_option_list_is_drawn_as_the_last_page() {
    let metrics = CLARA_BW_METRICS;
    let per_page = options_per_page(&metrics, true);
    let last = options(per_page + 2, None, 1).layout_for(&metrics);
    let beyond = options(per_page + 2, None, 40).layout_for(&metrics);
    assert_eq!(last, beyond);
}

#[test]
fn a_list_that_fits_one_page_has_no_pager() {
    let layout = options(3, Some(1), 0).layout();
    assert!(kinds(&layout, |kind| matches!(kind, LayoutKind::PagePosition)).is_empty());
    assert_eq!(
        kinds(&layout, |kind| matches!(kind, LayoutKind::OptionRow(..))).len(),
        3
    );
}

/// Lays out the middle page of an option list three pages long, under a top
/// bar and over a bottom action, measured with the status band a reader draws,
/// and says what went wrong, if anything.
fn option_list_over_a_bottom_action(metrics: &DisplayMetrics, summaries: bool) -> Option<String> {
    let count = options_per_page(metrics, summaries) * 2 + 1;
    let rows = (0..count).map(|index| {
        OptionRow::new(
            ActionId(100 + index as u32),
            format!("Zone {index}"),
            if summaries {
                format!("UTC+{index}")
            } else {
                String::new()
            },
        )
    });
    let screen = screen(vec![Node::Options {
        id: NodeId(1),
        prompt: "Time zone".to_owned(),
        options: rows.collect(),
        selected: Some(0),
        page: 1,
        previous: ActionId(60),
        next: ActionId(61),
    }])
    .with_bottom_action(BottomAction::new(
        NodeId(91),
        BarAction::new(ActionId(70), "Done"),
    ));
    let diagnostics = screen.diagnostics(metrics, &Chrome::measuring(true));
    if !diagnostics.issues.is_empty() {
        return Some(format!("{:?}", diagnostics.issues));
    }
    let pager = kinds(&diagnostics.layout, |kind| {
        matches!(kind, LayoutKind::PagePosition)
    })[0]
        .rect;
    let done = diagnostics.layout.rect_of_action(ActionId(70))?;
    (pager.y + pager.height > done.y).then(|| format!("the pager {pager:?} runs into {done:?}"))
}

#[test]
fn an_option_list_leaves_room_for_a_bottom_action_on_every_panel_at_every_size_held_either_way() {
    for (name, panel) in PANELS {
        for text_scale in TextScale::STEPS {
            for orientation in [Orientation::Portrait, Orientation::Landscape] {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..panel
                }
                .oriented(orientation);
                for summaries in [false, true] {
                    if let Some(problem) = option_list_over_a_bottom_action(&metrics, summaries) {
                        panic!("{name} {text_scale:?} {orientation:?} summaries {summaries}: {problem}");
                    }
                }
            }
        }
    }
}

/// The rows a page holds change by one every row's height of panel, so a
/// count that forgets a few pixels of chrome is right on most panels and
/// wrong on the few whose spare room is smaller than what it forgot. Every
/// height across more than a row's pitch finds those few, whatever the type
/// measures; held sideways, a Clara at Larger was one of them, and its pager
/// ran two pixels under the bottom action.
#[test]
fn an_option_list_with_a_bottom_action_fits_a_panel_of_any_height() {
    for text_scale in [TextScale::Default, TextScale::Larger, TextScale::Largest] {
        for height in 1000..1200 {
            let metrics = DisplayMetrics {
                width: 1448,
                height,
                pixels_per_inch: 300,
                text_scale,
            };
            for summaries in [false, true] {
                if let Some(problem) = option_list_over_a_bottom_action(&metrics, summaries) {
                    panic!("{height} px high, {text_scale:?}, summaries {summaries}: {problem}");
                }
            }
        }
    }
}

fn field(label: Option<&str>, hint: Option<&str>, error: Option<&str>, kind: FieldKind) -> Screen {
    screen(vec![
        Node::Field {
            id: NodeId(1),
            action: ActionId(2),
            value: "fake-token-abcdefghijklmn".to_owned(),
            placeholder: "Token".to_owned(),
            clear: Some(ActionId(3)),
            label: label.map(str::to_owned),
            hint: hint.map(str::to_owned),
            error: error.map(str::to_owned),
            kind,
        },
        Node::Button {
            id: NodeId(4),
            action: ActionId(5),
            label: "Save".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Primary,
        },
    ])
}

fn button_top(layout: &Layout) -> i32 {
    kinds(layout, |kind| matches!(kind, LayoutKind::Button(..)))[0]
        .rect
        .y
}

#[test]
fn a_field_without_label_hint_or_error_is_laid_out_as_it_always_was() {
    for (name, metrics) in PANELS {
        let plain = field(None, None, None, FieldKind::Text).layout_for(&metrics);
        let border = kinds(&plain, |kind| matches!(kind, LayoutKind::Field(..)))[0].rect;
        assert_eq!(
            border.height,
            metrics
                .touch_target_default()
                .max(metrics.touch_target_minimum()),
            "{name}"
        );
        assert_eq!(
            button_top(&plain),
            border.y + border.height + metrics.space(Space::Tight)
        );
    }
}

#[test]
fn an_error_in_place_of_a_hint_moves_nothing_on_the_panel() {
    for (name, metrics) in PANELS {
        let hinted = field(
            Some("Lichess token"),
            Some("From lichess.org, under API access tokens."),
            None,
            FieldKind::Text,
        )
        .layout_for(&metrics);
        let refused = field(
            Some("Lichess token"),
            Some("From lichess.org, under API access tokens."),
            Some("Lichess refused this token. Make a new one."),
            FieldKind::Text,
        )
        .layout_for(&metrics);
        assert_eq!(button_top(&hinted), button_top(&refused), "{name}");
        let note = kinds(&refused, |kind| matches!(kind, LayoutKind::FieldNote(_)))[0];
        assert_eq!(note.kind, LayoutKind::FieldNote(true));
        assert!(
            note.text_lines.join(" ").starts_with("Lichess refused"),
            "{name}"
        );
        let label = kinds(&refused, |kind| matches!(kind, LayoutKind::FieldLabel))[0];
        let border = kinds(&refused, |kind| matches!(kind, LayoutKind::Field(..)))[0].rect;
        assert!(label.rect.y + label.rect.height <= border.y, "{name}");
        assert!(note.rect.y >= border.y + border.height, "{name}");
    }
}

#[test]
fn a_secret_field_never_lays_out_what_it_holds() {
    let layout = field(None, None, None, FieldKind::Secret).layout();
    assert!(layout
        .nodes
        .iter()
        .all(|node| node.text_lines.iter().all(|line| !line.contains("lip_"))));
    let dots = kinds(&layout, |kind| matches!(kind, LayoutKind::FieldSecret(_)));
    assert_eq!(dots[0].kind, LayoutKind::FieldSecret(25));
    let plain = field(None, None, None, FieldKind::Text).layout();
    assert_eq!(button_top(&plain), button_top(&layout));
}

fn banner(level: BannerLevel, action: bool) -> Screen {
    screen(vec![Node::Banner {
        id: NodeId(1),
        level,
        text: "Lichess did not answer. Your games are still on the reader.".to_owned(),
        action: action.then(|| BarAction::new(ActionId(8), "Check again")),
    }])
}

#[test]
fn an_info_banner_without_an_action_is_the_height_it_always_was() {
    for (name, metrics) in PANELS {
        let layout = banner(BannerLevel::Info, false).layout_for(&metrics);
        let rect = kinds(&layout, |kind| matches!(kind, LayoutKind::Banner(_)))[0].rect;
        assert_eq!(
            rect.height,
            banner_height(
                "Lichess did not answer. Your games are still on the reader.",
                rect.width,
                &metrics
            ),
            "{name}"
        );
    }
}

#[test]
fn a_banner_action_sits_under_the_words_and_answers_a_tap() {
    for (name, metrics) in PANELS {
        let layout = banner(BannerLevel::Error, true).layout_for(&metrics);
        let band = kinds(&layout, |kind| matches!(kind, LayoutKind::Banner(_)))[0].clone();
        let action = kinds(&layout, |kind| matches!(kind, LayoutKind::BannerAction(_)))[0].rect;
        let words_bottom = band.rect.y
            + metrics.space(Space::Small)
            + i32::try_from(band.text_lines.len()).unwrap()
                * with_text_scale(metrics.text_scale, || FontSize::Body.line_height());
        assert!(action.y >= words_bottom, "{name}");
        assert!(
            action.y + action.height <= band.rect.y + band.rect.height,
            "{name}"
        );
        assert!(action.height >= metrics.touch_target_minimum(), "{name}");
        let (x, y) = centre(action);
        assert_eq!(layout.hit_test(x, y), Some(ActionId(8)), "{name}");
    }
}

#[test]
fn success_and_error_banners_draw_differently_from_info_and_from_each_other() {
    let metrics = CLARA_BW_METRICS;
    let info = painted(&banner(BannerLevel::Info, false), &metrics).pixels;
    let success = painted(&banner(BannerLevel::Success, false), &metrics).pixels;
    let error = painted(&banner(BannerLevel::Error, false), &metrics).pixels;
    assert_ne!(info, success);
    assert_ne!(info, error);
    assert_ne!(success, error);
}

fn code(payload: &str) -> Screen {
    screen(vec![Node::Code {
        id: NodeId(1),
        payload: payload.to_owned(),
        caption: "Scan to sign in at lichess.org".to_owned(),
    }])
}

#[test]
fn a_code_is_a_square_no_wider_than_its_column_or_fifty_millimetres() {
    for (name, metrics) in PANELS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = metrics.oriented(orientation);
            let layout = code("https://lichess.org/device").layout_for(&metrics);
            let square = kinds(&layout, |kind| matches!(kind, LayoutKind::Code))[0].rect;
            assert_eq!(square.width, square.height, "{name}");
            assert_eq!(
                square.width,
                metrics
                    .tenth_mm(CODE_SIDE_TENTH_MM)
                    .min(metrics.content_width()),
                "{name} {orientation:?}"
            );
        }
    }
}

#[test]
fn the_code_drawn_is_the_code_encoded_module_for_module() {
    let metrics = CLARA_BW_METRICS;
    let payload = "https://lichess.org/account/oauth/token/create?scopes[]=board:play";
    let shown = code(payload);
    let square = kinds(&shown.layout_for(&metrics), |kind| {
        matches!(kind, LayoutKind::Code)
    })[0]
        .rect;
    let surface = painted(&shown, &metrics);
    let encoded = encode_code(payload).expect("fits");
    let size = encoded.size();
    let total = size + CODE_QUIET_MODULES * 2;
    let module = square.width / total;
    assert!(
        module >= 6,
        "a module of {module} pixels is too small to scan"
    );
    let left = square.x + (square.width - module * total) / 2 + CODE_QUIET_MODULES * module;
    let top = square.y + (square.height - module * total) / 2 + CODE_QUIET_MODULES * module;
    for row in 0..size {
        for column in 0..size {
            let x = left + column * module + module / 2;
            let y = top + row * module + module / 2;
            let pixel = surface.pixels[y as usize * surface.width + x as usize];
            let dark = encoded.get_module(column, row);
            assert_eq!(pixel, if dark { tone::INK } else { tone::PAPER });
        }
    }
    // The quiet zone is paper all the way round.
    for offset in 0..CODE_QUIET_MODULES * module {
        let pixel = surface.pixels[(top - 1 - offset.min(top - square.y - 1)) as usize
            * surface.width
            + (left + module / 2) as usize];
        assert_eq!(pixel, tone::PAPER);
    }
}

#[test]
fn the_longest_payload_allowed_still_fits_a_version_ten_code() {
    let longest = "\u{e9}".repeat(MAX_CODE_BYTES / 2) + "x";
    assert_eq!(longest.len(), MAX_CODE_BYTES);
    let code = encode_code(&longest).expect("the limit fits");
    assert!(code.size() <= 57, "version {}", (code.size() - 17) / 4);
    assert!(encode_code(&format!("{longest}y")).is_none());
    assert!(encode_code("").is_none());
}

#[test]
fn a_code_that_cannot_be_drawn_is_an_error_before_it_reaches_the_panel() {
    let metrics = CLARA_BW_METRICS;
    for payload in [String::new(), "x".repeat(MAX_CODE_BYTES + 1)] {
        let issues = code(&payload).validate(&metrics);
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::CodeUnreadable
                    && issue.severity == DiagnosticSeverity::Error),
            "{issues:?}"
        );
    }
    assert!(code("https://lichess.org").validate(&metrics).is_empty());
}

fn steps(current: u8, total: u8, labels: &[&str]) -> Screen {
    screen(vec![Node::Steps {
        id: NodeId(1),
        current,
        total,
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
    }])
}

#[test]
fn a_step_indicator_is_one_line_and_names_the_step_it_is_on() {
    for (name, metrics) in PANELS {
        let layout =
            steps(1, 4, &["Account", "Sign in", "Choose a board", "Done"]).layout_for(&metrics);
        let line = kinds(&layout, |kind| matches!(kind, LayoutKind::Steps(..)))[0];
        assert_eq!(line.kind, LayoutKind::Steps(1, 4));
        assert_eq!(
            line.rect.height,
            with_text_scale(metrics.text_scale, || FontSize::Caption.line_height()),
            "{name}"
        );
        assert_eq!(line.text_lines, vec!["Step 2 of 4: Sign in".to_owned()]);
    }
    let unnamed = steps(9, 3, &[]).layout();
    let line = kinds(&unnamed, |kind| matches!(kind, LayoutKind::Steps(..)))[0];
    assert_eq!(line.kind, LayoutKind::Steps(2, 3));
    assert_eq!(line.text_lines, vec!["Step 3 of 3".to_owned()]);
}

#[test]
fn every_new_control_lays_out_without_an_error_on_every_panel() {
    let all = vec![
        toggle("Sleep", true, ControlState::Enabled),
        Node::Rows {
            id: NodeId(2),
            rows: vec![Row::new(ActionId(9), "Birds", "", Glyph::Book)
                .checked(true)
                .with_badge(3)
                .with_button(ActionId(10), "Open")],
        },
        Node::SelectionBar {
            id: NodeId(3),
            count: 2,
            actions: vec![
                BarAction::new(ActionId(11), "Delete"),
                BarAction::new(ActionId(12), "Move"),
            ],
        },
        Node::Steps {
            id: NodeId(4),
            current: 0,
            total: 3,
            labels: Vec::new(),
        },
    ];
    for (name, metrics) in PANELS {
        let issues = screen(all.clone()).diagnostics(&metrics, &Chrome::measuring(true));
        assert!(!issues.has_errors(), "{name}: {:?}", issues.issues);
    }
}

fn waveform_between(before: &Screen, after: &Screen) -> PanelWaveform {
    let metrics = CLARA_BW_METRICS;
    let mut planner = FramePlanner::new(metrics.width as usize, metrics.height as usize);
    let first = painted(before, &metrics);
    let transition = planner.plan(&first).expect("the first frame refreshes");
    assert!(planner.commit(&first, transition));
    planner
        .plan(&painted(after, &metrics))
        .expect("the change refreshes")
        .waveform
}

#[test]
fn flipping_a_switch_or_ticking_a_box_is_a_fast_two_level_refresh() {
    assert_eq!(
        waveform_between(
            &screen(vec![toggle("Sleep", false, ControlState::Enabled)]),
            &screen(vec![toggle("Sleep", true, ControlState::Enabled)]),
        ),
        PanelWaveform::Du
    );
    for (from, to) in [
        (RowState::Unchecked, RowState::Checked),
        (RowState::Checked, RowState::Unchecked),
        (RowState::Unchecked, RowState::Mixed),
    ] {
        assert_eq!(
            waveform_between(&rows_with(from, None, false), &rows_with(to, None, false)),
            PanelWaveform::Du,
            "{from:?} to {to:?}"
        );
    }
    let options = |selected| {
        screen(vec![Node::Options {
            id: NodeId(1),
            prompt: String::new(),
            options: vec![
                OptionRow::new(ActionId(2), "Stable", ""),
                OptionRow::new(ActionId(3), "Beta", ""),
            ],
            selected: Some(selected),
            page: 0,
            previous: ActionId(4),
            next: ActionId(5),
        }])
    };
    assert_eq!(
        waveform_between(&options(0), &options(1)),
        PanelWaveform::Du
    );
}

#[test]
fn a_banner_is_measured_at_the_height_it_is_drawn_whatever_its_level_and_action() {
    for (name, metrics) in PANELS {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale,
                ..metrics
            };
            for level in [
                BannerLevel::Info,
                BannerLevel::Attention,
                BannerLevel::Success,
                BannerLevel::Error,
            ] {
                for action in [false, true] {
                    let layout = banner(level, action).layout_for(&metrics);
                    let rect = kinds(&layout, |kind| matches!(kind, LayoutKind::Banner(_)))[0].rect;
                    assert_eq!(
                        rect.height,
                        banner_height_with(
                            "Lichess did not answer. Your games are still on the reader.",
                            level,
                            action,
                            rect.width,
                            &metrics
                        ),
                        "{name} {text_scale:?} {level:?} action {action}"
                    );
                }
            }
        }
    }
}

/// A row with something at its edge, built the way `row_button`,
/// `row_badge`, `rows_with_menu` and `rows_with_trailing` build it.
fn edged_row(title: &str, edge: RowEdge) -> Row {
    let mut row = Row::new(ActionId(1), title, "Updated at 10:40", Glyph::Book);
    if edge.menu {
        row = row.with_menu(ActionId(2));
    }
    if let Some(label) = edge.button {
        row.button = Some(BarAction::new(ActionId(3), label));
    }
    if edge.badge {
        row.badge = Some(12);
    }
    if !edge.trailing.is_empty() {
        row = row.with_trailing(edge.trailing);
    }
    row
}

#[test]
fn a_row_title_is_measured_at_the_width_it_is_drawn_beside_a_button_a_badge_a_menu_and_a_value() {
    let edges = [
        RowEdge::new(),
        RowEdge::new().with_menu(),
        RowEdge::new().with_badge(),
        RowEdge::new().with_button("Install"),
        RowEdge::new().with_menu().with_badge(),
        RowEdge::new()
            .with_button("Update")
            .with_badge()
            .with_trailing("2.1 MB"),
        RowEdge::new()
            .with_menu()
            .with_button("Open")
            .with_badge()
            .with_trailing("10:40"),
    ];
    for (name, metrics) in PANELS {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale,
                ..metrics
            };
            for edge in edges {
                let layout = screen(vec![Node::Rows {
                    id: NodeId(1),
                    rows: vec![edged_row("The Wren, the Robin and the Long Winter", edge)],
                }])
                .layout_for(&metrics);
                let title = kinds(&layout, |kind| matches!(kind, LayoutKind::RowTitle))[0].rect;
                // Measured against the row as wide as it was drawn. The
                // paginators measure against the readable measure, which on
                // a wide panel is narrower than the row and so errs towards
                // a line too many, never a line too few.
                let row = kinds(&layout, |kind| matches!(kind, LayoutKind::Row(_)))[0].rect;
                let area = ProseArea {
                    width: row.width,
                    ..metrics.prose_area(true, false)
                };
                assert_eq!(
                    title.width,
                    with_text_scale(text_scale, || row_title_width_at_edge(&metrics, area, edge)),
                    "{name} {text_scale:?} {edge:?}"
                );
            }
        }
    }
}

#[test]
fn rows_with_buttons_and_badges_are_paged_so_that_every_page_is_drawn_whole() {
    let titles = [
        "Feeds",
        "The Wren, the Robin and the Long Winter: notes from a garden",
        "Hacker News",
        "A very long subscription title that wraps onto a second line beside a badge",
        "Short",
        "Another feed with a title long enough to wrap when a button sits beside it",
    ];
    for (name, metrics) in PANELS {
        for text_scale in [TextScale::Default, TextScale::Larger, TextScale::Largest] {
            let metrics = DisplayMetrics {
                text_scale,
                ..metrics
            };
            let rows: Vec<(&str, &str, RowEdge)> = (0..30)
                .map(|index| {
                    let edge = match index % 3 {
                        0 => RowEdge::new().with_menu().with_badge(),
                        1 => RowEdge::new().with_button("Install"),
                        _ => RowEdge::new(),
                    };
                    (titles[index % titles.len()], "Updated at 10:40", edge)
                })
                .collect();
            let area = metrics.prose_area(true, false);
            let pages = with_text_scale(text_scale, || {
                paginate_rows_with_edges(&rows, &metrics, area)
            });
            let mut seen = Vec::new();
            for page in &pages {
                let built = screen(vec![Node::Rows {
                    id: NodeId(1),
                    rows: page
                        .iter()
                        .map(|&index| edged_row(rows[index].0, rows[index].2))
                        .collect(),
                }]);
                let layout = built.layout_for(&metrics);
                let drawn = kinds(&layout, |kind| matches!(kind, LayoutKind::Row(_)));
                let top = drawn.iter().map(|node| node.rect.y).min().unwrap_or(0);
                let bottom = drawn
                    .iter()
                    .map(|node| node.rect.y + node.rect.height)
                    .max()
                    .unwrap_or(0);
                assert_eq!(drawn.len(), page.len(), "{name} {text_scale:?}: {page:?}");
                assert!(
                    bottom - top <= area.height,
                    "{name} {text_scale:?}: page {page:?} runs {} px past the area",
                    bottom - top - area.height
                );
                seen.extend(page.iter().copied());
            }
            assert_eq!(
                seen,
                (0..rows.len()).collect::<Vec<_>>(),
                "{name} {text_scale:?}"
            );
        }
    }
}

#[test]
fn rows_measured_without_their_buttons_and_badges_run_past_the_page_somewhere() {
    // Why `paginate_rows_with_edges` exists: the same rows paged as though
    // only the menu stood at their edge overflow on at least one panel.
    let title = "A subscription title long enough to wrap once a badge and a button sit beside it";
    let mut overflowed = false;
    for (_, metrics) in PANELS {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale,
                ..metrics
            };
            let edge = RowEdge::new()
                .with_menu()
                .with_button("Install")
                .with_badge();
            let rows: Vec<(&str, &str)> = (0..30).map(|_| (title, "Updated at 10:40")).collect();
            let area = metrics.prose_area(true, false);
            let pages = with_text_scale(text_scale, || {
                paginate_rows_with_menu(&rows, &metrics, area)
            });
            for page in &pages {
                let layout = screen(vec![Node::Rows {
                    id: NodeId(1),
                    rows: page.iter().map(|_| edged_row(title, edge)).collect(),
                }])
                .layout_for(&metrics);
                let drawn = kinds(&layout, |kind| matches!(kind, LayoutKind::Row(_)));
                let top = drawn.iter().map(|node| node.rect.y).min().unwrap_or(0);
                let bottom = drawn
                    .iter()
                    .map(|node| node.rect.y + node.rect.height)
                    .max()
                    .unwrap_or(0);
                overflowed |= drawn.len() < page.len() || bottom - top > area.height;
            }
        }
    }
    assert!(overflowed);
}
