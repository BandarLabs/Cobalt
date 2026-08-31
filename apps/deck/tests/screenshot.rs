use kobo_image::encode_png_grey;
use kobo_sdk::{Glyph, ScreenBuilder};
use kobo_ui::{render, Surface, CLARA_BW_METRICS};
#[test]
fn writes_clean_clara_bw_capture() {
    let screen = ScreenBuilder::new("deck-address").top_bar("Deck").heading("Pair with your computer").text("Run kobo-sidekickd init, then type its address. Deck uses the existing Sidekick pairing.").splash(Some(Glyph::Grid), "No keys", "Pairing keeps commands on the computer.").build();
    let mut surface = Surface::new(
        CLARA_BW_METRICS.width as usize,
        CLARA_BW_METRICS.height as usize,
    );
    render(&screen, &mut surface, None);
    let png =
        encode_png_grey(surface.width as u32, surface.height as u32, &surface.pixels).expect("png");
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/screenshots/deck.png"),
        png,
    )
    .expect("write screenshot");
}
