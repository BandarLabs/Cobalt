use super::*;
use kobo_sdk::{AppRunner, KoboApp};

#[derive(Default)]
struct App {
    view: Option<ComicView>,
}
impl KoboApp for App {
    fn on_start(&mut self, context: &mut Context) {
        let view = ComicView::open(
            context,
            include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec(),
            "A walk in the rain",
        )
        .unwrap();
        context.set_screen(view.screen());
        self.view = Some(view);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let view = self.view.as_mut().unwrap();
        assert_ne!(view.act(context, action), Outcome::Elsewhere);
        context.set_screen(view.screen());
    }
}

#[test]
fn controls_pages_numeric_jump_and_pan_fit_supported_profiles_and_scales() {
    for profile in kobo_profile::SUPPORTED_PROFILES {
        for scale in [
            kobo_ui::TextScale::Default,
            kobo_ui::TextScale::Large,
            kobo_ui::TextScale::ExtraLarge,
        ] {
            let metrics = DisplayMetrics {
                width: i32::try_from(profile.width).unwrap(),
                height: i32::try_from(profile.height).unwrap(),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: scale,
            };
            kobo_text::install(metrics).unwrap();
            let mut runner = AppRunner::with_metrics(App::default(), metrics);
            runner.start();
            runner.app_mut().view.as_mut().unwrap().notice =
                Some("Saved reading position could not be read. Choose a page to continue.".into());
            runner
                .app_mut()
                .view
                .as_mut()
                .unwrap()
                .set_save_state(SaveState::Failed);
            for action in [
                "comic-controls",
                "comic-details",
                "comic-read",
                "comic-controls",
                "comic-save-retry",
                "comic-pages",
                "comic-thumbs-next",
                "comic-page-0",
                "comic-controls",
                "comic-jump",
                "comic-digit-4",
                "comic-go",
                "comic-controls",
                "comic-pan",
                "comic-up",
                "comic-down",
                "comic-read",
                "comic-controls",
                "comic-options",
                "comic-rotate",
                "comic-controls",
                "comic-pages",
                "comic-read",
                "comic-controls",
                "comic-jump",
            ] {
                let view = runner.app_mut().view.as_ref().unwrap();
                let screen = view.screen();
                let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                let layout = screen.layout_with(&view.metrics, &chrome);
                let rect = layout
                    .rect_of_action(action_id(action))
                    .unwrap_or_else(|| panic!("{} {scale:?}: {action} is missing", profile.id));
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action_id(action)),
                    "{} {scale:?}: {action} is unreachable",
                    profile.id
                );
                runner.action(action_id(action));
                let view = runner.app_mut().view.as_ref().unwrap();
                let screen = view.screen();
                let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                let diagnostics = screen.diagnostics(&view.metrics, &chrome);
                let serious = diagnostics
                    .issues
                    .iter()
                    .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                    .collect::<Vec<_>>();
                assert!(
                    serious.is_empty(),
                    "{} {:?} {action}: {serious:?}",
                    profile.id,
                    scale
                );
            }
        }
    }
}

#[test]
fn colour_page_larger_than_wire_budget_is_resized_without_losing_its_channels() {
    struct ColourApp;
    impl KoboApp for ColourApp {
        fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        fn on_start(&mut self, context: &mut Context) {
            let picture =
                kobo_image::Picture::from_rgb(1500, 1500, [210, 40, 90].repeat(1500 * 1500))
                    .unwrap();
            assert!(put_page(context, PAGE, picture).is_some());
        }
    }
    let commands = AppRunner::new(ColourApp).start();
    let (width, height, pixels) = commands
        .iter()
        .find_map(|command| match command {
            kobo_sdk::Command::PutPicture {
                width,
                height,
                pixels,
                format: kobo_sdk::PictureFormat::Rgb,
                ..
            } => Some((*width, *height, pixels)),
            _ => None,
        })
        .expect("RGB picture sent");
    assert_eq!(width, height);
    assert!(width < 1500);
    assert!(pixels.len() <= kobo_sdk::MAX_PICTURE_BYTES);
    assert!(pixels.chunks_exact(3).all(|pixel| pixel == [210, 40, 90]));
}
