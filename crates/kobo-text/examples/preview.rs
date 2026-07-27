//! Renders launcher screens to PGM files so type can be judged by eye.
//!
//! Looking at the panel is the only real test of a typeface, but a headless
//! render catches the gross failures first and costs the device nothing.

use kobo_sdk::{Glyph, ScreenBuilder};
use kobo_ui::{render_with, Chrome, DisplayMetrics, Screen, Surface, CLARA_BW_METRICS};

fn home() -> Screen {
    ScreenBuilder::new("launcher")
        .top_bar("Applications")
        .rows([
            (
                "hello",
                "Hello",
                "The smallest application: one heading and one button.",
                Glyph::App,
            ),
            (
                "gallery",
                "Components",
                "Every UI primitive on real hardware, for checking by eye.",
                Glyph::Chart,
            ),
            (
                "counter",
                "Counter",
                "State, actions and partial repaints.",
                Glyph::Note,
            ),
        ])
        .divider()
        .button("reader", "Return to Kobo reader")
        .build()
}

fn reading() -> Screen {
    ScreenBuilder::new("reading")
        .top_bar("Library")
        .heading("Pride and Prejudice")
        .text("It is a truth universally acknowledged, that a single man in possession of a good fortune, must be in want of a wife.")
        .divider()
        .text("Downloaded from Project Gutenberg while the reader was running.")
        .button("open", "Open book")
        .build()
}

fn write(name: &str, screen: &Screen, metrics: &DisplayMetrics) {
    // Previews are build output, so they go under `target` rather than beside
    // the source where they would be committed by accident.
    let directory = std::path::Path::new("target/previews");
    std::fs::create_dir_all(directory).expect("make the preview directory");
    let name = directory.join(name);
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(screen, metrics, &Chrome::default(), &mut surface, None);
    let mut out = format!("P5\n{} {}\n255\n", surface.width, surface.height).into_bytes();
    out.extend_from_slice(&surface.pixels);
    std::fs::write(&name, out).expect("write the preview");
    println!("wrote {}", name.display());
}

fn main() {
    let metrics = CLARA_BW_METRICS;
    let installed = kobo_text::install(metrics);
    match &installed {
        Ok(path) => println!("typeface: {}", path.display()),
        Err(error) => println!("typeface: falling back to the bitmap ({error})"),
    }
    write("preview-launcher.pgm", &home(), &metrics);
    write("preview-reading.pgm", &reading(), &metrics);
}
