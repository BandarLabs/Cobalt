use super::*;

#[test]
fn landscape_rotates_rgb_and_luminance_together_and_clears_old_colour() {
    let mut logical = Surface::new(2, 3);
    for y in 0..3 {
        for x in 0..2 {
            logical.blend_colour(x, y, [(x * 70 + 20) as u8, (y * 60 + 15) as u8, 190], 255);
        }
    }
    for turn in [LandscapeTurn::Clockwise, LandscapeTurn::CounterClockwise] {
        let mut physical = Surface::new(3, 2);
        rotate_landscape(&logical, &mut physical, turn);
        for y in 0..3 {
            for x in 0..2 {
                let (px, py) = match turn {
                    LandscapeTurn::Clockwise => (2 - y, x),
                    LandscapeTurn::CounterClockwise => (y, 1 - x),
                };
                assert_eq!(physical.rgb_at(py * 3 + px), logical.rgb_at(y * 2 + x));
                assert_eq!(physical.pixels[py * 3 + px], logical.pixels[y * 2 + x]);
            }
        }
        let mut grey = Surface::new(2, 3);
        grey.clear(128);
        rotate_landscape(&grey, &mut physical, turn);
        assert!(physical.chroma.is_none());
        assert_eq!(physical.pixels, vec![128; 6]);
    }
}
