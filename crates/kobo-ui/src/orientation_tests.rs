use super::*;

#[test]
fn landscape_swaps_metrics_and_maps_both_physical_sides() {
    let landscape = CLARA_BW_METRICS.oriented(Orientation::Landscape);
    assert_eq!((landscape.width, landscape.height), (1448, 1072));
    assert_eq!(
        logical_point_with_turn(
            Orientation::Landscape,
            LandscapeTurn::Clockwise,
            1072,
            1448,
            1071,
            0,
        ),
        (0, 0)
    );
    assert_eq!(
        logical_point_with_turn(
            Orientation::Landscape,
            LandscapeTurn::CounterClockwise,
            1072,
            1448,
            0,
            1447,
        ),
        (0, 0)
    );
}

#[test]
fn software_rotation_places_every_corner_for_both_landscape_sides() {
    let logical = Surface {
        width: 3,
        height: 2,
        pixels: vec![1, 2, 3, 4, 5, 6],
        chroma: None,
    };
    let mut clockwise = Surface::new(2, 3);
    rotate_landscape(&logical, &mut clockwise, LandscapeTurn::Clockwise);
    assert_eq!(clockwise.pixels, [4, 1, 5, 2, 6, 3]);

    let mut counter_clockwise = Surface::new(2, 3);
    rotate_landscape(
        &logical,
        &mut counter_clockwise,
        LandscapeTurn::CounterClockwise,
    );
    assert_eq!(counter_clockwise.pixels, [3, 6, 2, 5, 1, 4]);
}
