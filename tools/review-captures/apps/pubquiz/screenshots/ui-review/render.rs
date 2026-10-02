use kobo_ui::{Chrome, DisplayMetrics, Screen, Surface, CLARA_BW_METRICS};
use std::io::Write;

pub fn capture(app: &str, name: &str, screen: &Screen) {
    capture_at(app, name, screen, CLARA_BW_METRICS, &());
}

pub fn capture_at(
    app: &str,
    name: &str,
    screen: &Screen,
    metrics: DisplayMetrics,
    pictures: &dyn kobo_ui::Pictures,
) {
    let phase = std::env::var("COBALT_REVIEW_PHASE").unwrap_or_else(|_| "before".into());
    let dir = std::path::PathBuf::from(
        std::env::var_os("COBALT_REVIEW_OUT").expect("set COBALT_REVIEW_OUT for capture output"),
    )
    .join(app)
    .join(phase);
    std::fs::create_dir_all(&dir).unwrap();
    let chrome = Chrome::for_screen(screen, false, Chrome::measuring(true).status);
    let screen = &kobo_ui::ensure_way_back(screen.clone(), &chrome, app);
    let mut surface = Surface::new(
        usize::try_from(metrics.width).unwrap(),
        usize::try_from(metrics.height).unwrap(),
    );
    kobo_ui::render_all(screen, &metrics, &chrome, pictures, &mut surface, None);
    let mut file = std::fs::File::create(dir.join(format!("{name}.pgm"))).unwrap();
    write!(file, "P5\n{} {}\n255\n", surface.width, surface.height).unwrap();
    file.write_all(&surface.pixels).unwrap();
    std::fs::write(dir.join(format!("{name}.txt")), format!("Renderer snapshot, not an interactive simulator capture.\nmetrics: {metrics:?}\nchrome: {chrome:?}\nowns_back: {}\nissues: {:#?}\nlayout: {:#?}\n", screen.owns_back, screen.diagnostics(&metrics,&chrome).issues, screen.layout_with(&metrics,&chrome))).unwrap();
}
