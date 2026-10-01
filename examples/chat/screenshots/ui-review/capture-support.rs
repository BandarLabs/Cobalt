use kobo_ui::{Chrome, DisplayMetrics, PictureCache, Screen, Surface, CLARA_BW_METRICS};
use std::path::Path;
fn save_capture(name: &str, screen: &Screen, metrics: DisplayMetrics, at_home: bool) {
    save_capture_cached(name, screen, metrics, at_home, &PictureCache::default());
}
fn save_capture_cached(
    name: &str,
    screen: &Screen,
    metrics: DisplayMetrics,
    at_home: bool,
    pictures: &PictureCache,
) {
    kobo_text::install(metrics).expect("install bundled simulator typeface");
    let status = Chrome::measuring(true).status;
    let chrome = Chrome::for_screen(screen, at_home, status);
    let screen = kobo_ui::ensure_way_back(screen.clone(), &chrome, "UI review");
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    kobo_ui::render_all(&screen, &metrics, &chrome, pictures, &mut surface, None);
    let root = std::env::var("COBALT_UI_CAPTURE_DIR").expect("capture directory");
    std::fs::create_dir_all(&root).expect("capture directory");
    let png =
        kobo_image::encode_png_grey(surface.width as u32, surface.height as u32, &surface.pixels)
            .expect("PNG encode");
    std::fs::write(Path::new(&root).join(format!("{name}.png")), png)
        .expect("write original renderer pixels");
    let diagnostics = screen.diagnostics(&metrics, &chrome);
    let notes = format!("Native in-process Kobo renderer snapshot; NOT an interactive simulator capture.\nProfile: Clara BW {}x{}, text {}%. Synthetic status strip: 00:00 / 50% / connected.\nApp screen and shared renderer.\nDiagnostics: {:#?}\n",metrics.width,metrics.height,metrics.text_scale.percent(),diagnostics.issues);
    std::fs::write(Path::new(&root).join(format!("{name}.txt")), notes)
        .expect("write capture provenance");
    println!(
        "{name}: {} issues, errors={}",
        diagnostics.issues.len(),
        diagnostics.has_errors()
    );
}

fn save_commands(
    name: &str,
    commands: Vec<kobo_sdk::Command>,
    metrics: DisplayMetrics,
    home: bool,
) {
    let mut pictures = PictureCache::default();
    for command in commands {
        match command {
            kobo_sdk::Command::PutPicture {
                handle,
                width,
                height,
                format,
                pixels,
            } => {
                pictures.put_report_with(handle, width, height, format, pixels);
            }
            kobo_sdk::Command::SetScreen(screen) => {
                save_capture_cached(name, &screen, metrics, home, &pictures)
            }
            _ => {}
        }
    }
}
