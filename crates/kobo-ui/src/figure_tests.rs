use super::*;

/// The fallback bitmap face is fixed-pitch, so this only says anything
/// with a real typeface installed. Every test binary that draws installs
/// one; the guard is here so that one that does not still passes.
fn digits_are_proportional() -> bool {
    digit_cell(FontSize::Caption, Face::Text)
        > measure_text_in("1", FontSize::Caption, Face::Text).0
}

#[test]
fn a_clock_is_the_same_width_at_every_minute_of_the_day() {
    let mut widths = std::collections::BTreeSet::new();
    for hour in 0..24 {
        for minute in 0..60 {
            widths.insert(figures_width(
                &format!("{hour:02}:{minute:02}"),
                FontSize::Caption,
                Face::Text,
            ));
        }
    }
    assert_eq!(
        widths.len(),
        1,
        "the clock takes {} different widths through a day: {widths:?}",
        widths.len()
    );
}

#[test]
fn a_figure_on_a_fixed_advance_is_wider_than_the_face_would_set_it() {
    if !digits_are_proportional() {
        return;
    }
    assert!(
        figures_width("11:11", FontSize::Caption, Face::Text)
            > measure_text_in("11:11", FontSize::Caption, Face::Text).0,
        "the narrowest digits were not padded at all"
    );
    assert_eq!(
        figures_width("00:00", FontSize::Caption, Face::Text),
        measure_text_in("00:00", FontSize::Caption, Face::Text).0,
        "the widest digits should need no padding"
    );
}

#[test]
fn only_the_digits_are_put_on_a_cell() {
    assert_eq!(
        figures_width("of", FontSize::Caption, Face::Text),
        measure_text_in("of", FontSize::Caption, Face::Text).0,
        "text with no digits in it was respaced"
    );
}

/// The layout, not just the arithmetic: the box the clock claims is the box    /// that gets repainted every minute, so it has to be the figure's width and
/// it has to be the same width whatever the time is.
#[test]
fn the_box_the_clock_claims_does_not_change_as_it_counts() {
    let clock_rect = |time: &str| {
        let chrome = Chrome {
            back: false,
            status: Some(Status::standard(
                time,
                Some(Percent::new(50)),
                false,
                Signal::Strong,
                true,
            )),
        };
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "body".into(),
                links: Vec::new(),
            }],
        );
        screen
            .layout_with(&CLARA_BW_METRICS, &chrome)
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::StatusClock))
            .expect("the band drew no clock")
            .rect
    };

    let reference = clock_rect("00:00");
    for time in ["07:59", "08:00", "11:11", "23:59", "10:38"] {
        assert_eq!(
            clock_rect(time).width,
            reference.width,
            "the clock claimed a different box at {time}"
        );
    }
    assert!(
        reference.width < CLARA_BW_METRICS.width / 3,
        "the clock is claiming a third of the band it does not draw in"
    );
}

/// A rank is drawn on the fixed advance and measured on it too. Measured
/// on the face's own spacing instead, a column sized for a proportional
/// eleven is far too narrow for the tabular one that gets drawn, and the
/// digits back out of their column into the title beside them.
#[test]
fn no_rank_is_wider_than_the_column_it_was_measured_for() {
    for highest in [1_u16, 6, 9, 10, 11, 30, 99, 100, 111] {
        let column = row_rank_column(&CLARA_BW_METRICS, highest);
        for rank in 1..=highest {
            assert!(
                figures_width(&rank.to_string(), FontSize::Caption, Face::Text) <= column,
                "rank {rank} does not fit the column measured for {highest}"
            );
        }
    }
}

#[test]
fn landscape_swaps_the_logical_viewport_and_inverts_touch_clockwise() {
    let landscape = CLARA_BW_METRICS.oriented(Orientation::Landscape);
    assert_eq!((landscape.width, landscape.height), (1448, 1072));
    assert_eq!(logical_point(Orientation::Landscape, 1072, 1071, 0), (0, 0));
    assert_eq!(
        logical_point(Orientation::Landscape, 1072, 0, 1447),
        (1447, 1071)
    );
    assert_eq!(
        logical_point(Orientation::Landscape, 1072, 536, 724),
        (724, 535)
    );
}

#[test]
fn landscape_rotation_maps_each_logical_corner_to_its_physical_corner() {
    let mut logical = Surface::new(3, 2);
    logical.pixels = vec![1, 2, 3, 4, 5, 6];
    let mut physical = Surface::new(2, 3);
    rotate_landscape(&logical, &mut physical, LandscapeTurn::Clockwise);
    assert_eq!(physical.pixels, vec![4, 1, 5, 2, 6, 3]);
}
