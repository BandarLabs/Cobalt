//! One scoped typography environment for measurement and painting.

use crate::{
    reading_scale, set_reading_scale, set_text_scale, text_scale, DisplayMetrics, Screen, TextScale,
};

/// Applies a screen's effective type sizes and restores the caller's sizes,
/// including when layout or painting unwinds. The guard cannot cross threads.
pub(crate) struct TextEnvironment {
    interface: TextScale,
    reading: TextScale,
    thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl TextEnvironment {
    pub(crate) fn enter(screen: &Screen, metrics: &DisplayMetrics) -> Self {
        let previous = Self {
            interface: text_scale(),
            reading: reading_scale(),
            thread: std::marker::PhantomData,
        };
        set_text_scale(metrics.text_scale);
        set_reading_scale(screen.text_scale.unwrap_or(metrics.text_scale));
        previous
    }
}

impl Drop for TextEnvironment {
    fn drop(&mut self) {
        set_text_scale(self.interface);
        set_reading_scale(self.reading);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chrome, Node, NodeId, Surface, CLARA_BW_METRICS};

    #[test]
    fn nested_environment_restores_both_scales_even_after_unwind() {
        crate::with_text_scale(TextScale::Smaller, || {
            crate::with_reading_scale(TextScale::Large, || {
                let screen = Screen::new(1, vec![]).with_text_scale(Some(TextScale::Largest));
                let mut metrics = CLARA_BW_METRICS;
                metrics.text_scale = TextScale::ExtraLarge;
                let result = std::panic::catch_unwind(|| {
                    let _scope = TextEnvironment::enter(&screen, &metrics);
                    assert_eq!(text_scale(), TextScale::ExtraLarge);
                    assert_eq!(reading_scale(), TextScale::Largest);
                    panic!("test unwind");
                });
                assert!(result.is_err());
                assert_eq!(text_scale(), TextScale::Smaller);
                assert_eq!(reading_scale(), TextScale::Large);
            });
        });
    }

    #[test]
    fn render_and_layout_use_explicit_metrics_without_ambient_setup() {
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "A readable line of text. ".repeat(80),
                links: vec![],
            }],
        );
        let regular = CLARA_BW_METRICS;
        let mut large = regular;
        large.text_scale = TextScale::ExtraLarge;
        let a = screen.layout_with(&regular, &Chrome::default());
        let b = screen.layout_with(&large, &Chrome::default());
        assert_ne!(a.nodes[0].rect.height, b.nodes[0].rect.height);
        let mut first = Surface::new(1072, 1448);
        let mut second = Surface::new(1072, 1448);
        crate::render_with(&screen, &regular, &Chrome::default(), &mut first, None);
        crate::render_with(&screen, &large, &Chrome::default(), &mut second, None);
        assert!(
            first.pixels != second.pixels,
            "different type scales must change the rendered page"
        );
    }
}
