use kobo_image::encode_png_grey;
use kobo_sdk::{Glyph, ScreenBuilder};
use kobo_ui::{render, Surface, CLARA_BW_METRICS};
#[test]
fn renders_clean_clara_bw_capture() {
    let screen = ScreenBuilder::new("vault-home")
        .top_bar("Vault")
        .splash(
            Some(Glyph::Note),
            "No vault yet",
            "Run kobo vault init, then kobo vault push ~/Notes.",
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
