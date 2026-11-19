use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn capture(name: &str, original: Screen, metrics: DisplayMetrics) {
    let Ok(root) = std::env::var("COBALT_REVIEW_CAPTURE_DIR") else {
        return;
    };
    let font = kobo_text::install(metrics).expect("real font");
    let chrome = Chrome::for_screen(&original, false, Chrome::measuring(true).status);
    let screen = kobo_ui::ensure_way_back(
        original,
        &chrome,
        env!("CARGO_PKG_NAME").trim_start_matches("kobo-"),
    );
    let mut surface = kobo_ui::Surface::new(
        usize::try_from(metrics.width).unwrap(),
        usize::try_from(metrics.height).unwrap(),
    );
    kobo_ui::render_with(&screen, &metrics, &chrome, &mut surface, None);
    let png = kobo_image::encode_png_grey(
        u32::try_from(metrics.width).unwrap(),
        u32::try_from(metrics.height).unwrap(),
        &surface.pixels,
    )
    .unwrap();
    let directory =
        std::path::Path::new(&root).join(env!("CARGO_PKG_NAME").trim_start_matches("kobo-"));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{name}-{}.png", metrics.text_scale.percent())),
        png,
    )
    .unwrap();
    let evidence = format!("Capture: genuine app screen builder + Cobalt renderer; NOT a live simulator capture.\nScenario: {name}\nSource base: 97153048\nShell: synthetic measuring status strip, runtime ensure_way_back\nFont: {font:?}\nFont roles: {:?}\nMetrics: {metrics:?}\nScreen: {screen:#?}\nLayout: {:#?}\nDiagnostics: {:#?}\n", kobo_text::installed_sources(), screen.layout_with(&metrics, &chrome), screen.diagnostics(&metrics, &chrome));
    std::fs::write(
        directory.join(format!("{name}-{}.txt", metrics.text_scale.percent())),
        evidence,
    )
    .unwrap();
}
fn metrics() -> Vec<DisplayMetrics> {
    [
        TextScale::Default,
        TextScale::ExtraLarge,
        TextScale::Largest,
    ]
    .into_iter()
    .map(|text_scale| DisplayMetrics {
        text_scale,
        ..CLARA_BW_METRICS
    })
    .collect()
}
#[test]
fn capture_review_baselines() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(Needles::default(), metrics);
        capture("project", runner.app().screen(&runner.context()), metrics);
        runner.action(action_id("plus"));
        runner.action(action_id("plus"));
        runner.action(action_id("undo"));
        capture(
            "project-undo",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().projects = (0..12)
            .map(|i| Project::new(format!("Knitting project {}", i + 1)))
            .collect();
        runner.action(action_id("projects"));
        capture(
            "projects-twelve",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..12 {
            runner.action(action_id("list-next"));
        }
        capture(
            "projects-last",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().loaded = [true; 3];
        runner.app_mut().libraries[0] = (0..20)
            .map(|i| Pattern {
                title: format!("Winter pattern {}", i + 1),
                detail: "Local synthetic knitting pattern".into(),
            })
            .collect();
        runner.action(action_id("library"));
        capture(
            "library-twenty",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..20 {
            runner.action(action_id("list-next"));
        }
        capture(
            "library-last",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(action_id("sync"));
        capture(
            "sync-writing",
            runner.app().screen(&runner.context()),
            metrics,
        );
        let task = runner.app().task.expect("sync task").0;
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unauthorized));
        capture(
            "sync-failed",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().notice = None;
        runner.action(action_id("queue-tab"));
        capture(
            "queue-empty",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().notice =
            Some("Sign-in needed. Add your Ravelry account during setup.".into());
        capture(
            "library-signin",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}
