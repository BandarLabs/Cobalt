use crate::{ActionId, AppRunner, Context, DisplayMetrics, KoboApp};
use kobo_ui::TextScale;

#[derive(Default)]
struct Probe(Vec<(TextScale, TextScale)>);
impl KoboApp for Probe {
    fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
    fn on_start(&mut self, _context: &mut Context) {
        self.0
            .push((kobo_ui::text_scale(), kobo_ui::reading_scale()));
    }
}

#[test]
fn callbacks_measure_at_their_own_scale_and_restore_the_callers_environment() {
    let metrics = DisplayMetrics {
        text_scale: TextScale::ExtraLarge,
        ..kobo_ui::CLARA_BW_METRICS
    };
    kobo_ui::with_text_scale(TextScale::Smaller, || {
        kobo_ui::with_reading_scale(TextScale::Largest, || {
            let mut runner = AppRunner::with_metrics(Probe::default(), metrics);
            runner.start();
            assert_eq!(
                runner.app().0,
                [(TextScale::ExtraLarge, TextScale::ExtraLarge)]
            );
            assert_eq!(kobo_ui::text_scale(), TextScale::Smaller);
            assert_eq!(kobo_ui::reading_scale(), TextScale::Largest);
        });
    });
}
