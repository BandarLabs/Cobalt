use crate::{action_id, Chrome, Context, ScreenBuilder};
use kobo_ui::{render_with, LayoutKind, Surface};

#[test]
fn selected_keys_keep_their_label_geometry_and_gain_a_visible_outline() {
    let metrics = Context::default().metrics();
    #[cfg(feature = "text")]
    kobo_text::install(metrics).unwrap();
    let build = |selected| {
        ScreenBuilder::new("key-state")
            .grid_with_selection(
                3,
                false,
                [
                    ("one", "1", false),
                    ("two", "2", selected),
                    ("three", "3", false),
                ],
            )
            .build()
    };
    let plain = build(false);
    let selected = build(true);
    let chrome = Chrome::default();
    let before = plain.layout_with(&metrics, &chrome);
    let after = selected.layout_with(&metrics, &chrome);
    assert_eq!(
        before.rect_of_action(action_id("two")),
        after.rect_of_action(action_id("two"))
    );
    assert!(after.nodes.iter().any(|n|matches!(n.kind,LayoutKind::Cell(id,kobo_ui::CellStyle::Key,true) if id==action_id("two"))));
    assert!(selected.diagnostics(&metrics, &chrome).issues.is_empty());
    let render = |screen| {
        let mut pixels = Surface::new(
            usize::try_from(metrics.width).unwrap(),
            usize::try_from(metrics.height).unwrap(),
        );
        render_with(screen, &metrics, &chrome, &mut pixels, None);
        pixels
    };
    let before_pixels = render(&plain);
    let after_pixels = render(&selected);
    let bounds = after.rect_of_action(action_id("two")).unwrap();
    let mut changed = 0;
    for (index, (old, new)) in before_pixels
        .pixels
        .iter()
        .zip(&after_pixels.pixels)
        .enumerate()
    {
        if old != new {
            changed += 1;
            let x = i32::try_from(index % after_pixels.width).unwrap();
            let y = i32::try_from(index / after_pixels.width).unwrap();
            assert!(
                x >= bounds.x
                    && x < bounds.x + bounds.width
                    && y >= bounds.y
                    && y < bounds.y + bounds.height,
                "changed {x},{y}; bounds {bounds:?}"
            );
        }
    }
    assert!(changed > 0);
}
