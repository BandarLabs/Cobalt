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

#[test]
fn detached_context_pagination_preserves_words_and_fits_its_reader() {
    use crate::{Chrome, ScreenBuilder};
    let text = (0..180)
        .map(|n| format!("word{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..kobo_ui::CLARA_BW_METRICS
        };
        let context = AppRunner::with_metrics(Probe::default(), metrics).context();
        kobo_ui::with_text_scale(TextScale::Smaller, || {
            kobo_ui::with_reading_scale(TextScale::Largest, || {
                for reading in [false, true] {
                    let pages = if reading {
                        context.paginate_reading(&text, true)
                    } else {
                        context.paginate(&text, true)
                    };
                    let recovered = pages
                        .iter()
                        .flatten()
                        .flat_map(|p| p.split_whitespace())
                        .collect::<Vec<_>>()
                        .join(" ");
                    assert_eq!(recovered, text);
                    for (index, page) in pages.iter().enumerate() {
                        let mut builder = ScreenBuilder::new("detached-prose")
                            .top_bar("Reading")
                            .reading(reading)
                            .page_position(
                                u16::try_from(index + 1).expect("page"),
                                u16::try_from(pages.len()).expect("pages"),
                            )
                            .action_bar([("previous", "Previous"), ("next", "Next")]);
                        for paragraph in page {
                            builder = builder.text(paragraph);
                        }
                        let issues = builder
                            .build()
                            .diagnostics(&metrics, &Chrome::measuring(true))
                            .issues;
                        assert!(
                            issues.is_empty(),
                            "{text_scale:?}, reading={reading}: {issues:?}"
                        );
                    }
                }
                assert_eq!(kobo_ui::text_scale(), TextScale::Smaller);
                assert_eq!(kobo_ui::reading_scale(), TextScale::Largest);
            });
        });
    }
}
