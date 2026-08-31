use kobo_image::encode_png_grey;
use kobo_sdk::{Glyph, ScreenBuilder};
use kobo_ui::{render, Surface, CLARA_BW_METRICS};
#[test]
fn writes_clean_clara_bw_capture() {
    let screen = ScreenBuilder::new("hb-today")
        .top_bar("Habits")
        .tabs(
            0,
            [
                ("today", "Today"),
                ("streaks", "Streaks"),
                ("manage", "Manage"),
                ("stats", "Stats"),
                ("settings", "Settings"),
            ],
        )
        .rows([
            ("done-read", "Read", "daily", Glyph::Circle),
            ("done-walk", "Walk", "weekdays", Glyph::Check),
            ("done-write", "Write", "daily", Glyph::Circle),
        ])
        .build();
    let mut surface = Surface::new(
        CLARA_BW_METRICS.width as usize,
        CLARA_BW_METRICS.height as usize,
    );
    render(&screen, &mut surface, None);
    let png =
        encode_png_grey(surface.width as u32, surface.height as u32, &surface.pixels).expect("png");
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/screenshots/today.png"),
        png,
    )
    .expect("write screenshot");
}
