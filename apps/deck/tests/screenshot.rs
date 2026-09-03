use kobo_image::encode_png_grey;
use kobo_sdk::ScreenBuilder;
use kobo_ui::{render, Surface, CLARA_BW_METRICS};
#[test]
fn renders_clean_clara_bw_capture() {
    let screen = ScreenBuilder::new("deck-grid")
        .top_bar("Deck")
        .tabs(0, [("page-build", "Build")])
        .grid(
            2,
            false,
            [
                ("test", "Test · all packages"),
                ("format", "Format · workspace"),
                ("status", "Status · working tree"),
                ("docs", "Docs · local preview"),
                ("build", "Build · release"),
                ("deploy", "Deploy · staging"),
            ],
        )
        .build();
    let mut surface = Surface::new(
        CLARA_BW_METRICS.width as usize,
        CLARA_BW_METRICS.height as usize,
    );
    render(&screen, &mut surface, None);
    let width = u32::try_from(surface.width).expect("Clara BW width fits u32");
    let height = u32::try_from(surface.height).expect("Clara BW height fits u32");
    let png = encode_png_grey(width, height, &surface.pixels).expect("png");
    assert_eq!((width, height), (1072, 1448));
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
}
