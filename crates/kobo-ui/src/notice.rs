//! Where the runtime's notice card goes on a laid-out screen.
//!
//! Under the status band, over the top of the page, the width of the page's
//! own column. That is the part of a screen least likely to hold what the
//! owner is in the middle of: the bar's title is already known, and the
//! controls that matter most sit at the foot, which a card at the top never
//! covers, nor a keyboard rising from the bottom.

use super::{
    max, one_line, DisplayMetrics, FontSize, Layout, LayoutKind, LayoutNode, NodeId, NoticeCard,
    Rect, Space,
};

/// What happened is said small and grey above what it happened to, the way
/// a list says the day above its rows: the title is the news, and the card
/// should read like one more row of the system rather than a headline.
pub(super) const HEADING: FontSize = FontSize::Note;
pub(super) const LINE: FontSize = FontSize::Body;

/// The node id every part of the card carries. Not an application's: an
/// application's ids count up from zero and never reach the top of the range.
const CARD: NodeId = NodeId(u32::MAX);

pub(super) fn layout(card: &NoticeCard, metrics: &DisplayMetrics, band: i32, layout: &mut Layout) {
    let margin = metrics.screen_margin();
    let padding = metrics.space(Space::Small);
    let lift = max(2, metrics.rule_thickness() * 3);
    // The dialogue's width, so the two runtime sheets line up with each other
    // and with the column of words under them.
    let width = max(
        1,
        (metrics.width - 2 * margin).min(metrics.readable_width()),
    );
    let x = (metrics.width - width) / 2;
    let y = if band > 0 {
        band.saturating_add(metrics.space(Space::Tight))
    } else {
        margin
    };
    let heading_height = HEADING.line_height();
    let line_height = if card.line.is_empty() {
        0
    } else {
        LINE.line_height()
    };
    let words = heading_height + line_height;
    // The whole card is the target, so it is never shorter than a target is
    // allowed to be, even when it has only a heading to say.
    let inner = max(words, metrics.touch_target_default() - 2 * padding);
    // The mark stands as tall as the two lines beside it, like a tile's mark
    // beside its name, but no larger than a control: at the largest text
    // sizes a bigger picture says nothing more and takes the title's room.
    let mark = inner.min(metrics.touch_target_default());
    let text_x = x + padding + mark + padding;
    let text_width = max(1, width - lift - 3 * padding - mark);
    let height = inner + 2 * padding + lift;
    let top = y + padding + (inner - words) / 2;

    layout.nodes.push(LayoutNode {
        id: CARD,
        rect: Rect {
            x,
            y,
            width,
            height,
        },
        kind: LayoutKind::Notice,
        text_lines: Vec::new(),
    });
    layout.nodes.push(LayoutNode {
        id: CARD,
        rect: Rect {
            x: x + padding,
            y: y + padding + (inner - mark) / 2,
            width: mark,
            height: mark,
        },
        kind: LayoutKind::NoticeMark(card.glyph),
        text_lines: Vec::new(),
    });
    layout.nodes.push(LayoutNode {
        id: CARD,
        rect: Rect {
            x: text_x,
            y: top,
            width: text_width,
            height: heading_height,
        },
        kind: LayoutKind::NoticeHeading,
        text_lines: vec![one_line(&card.heading, text_width, HEADING)],
    });
    if !card.line.is_empty() {
        layout.nodes.push(LayoutNode {
            id: CARD,
            rect: Rect {
                x: text_x,
                y: top + heading_height,
                width: text_width,
                height: line_height,
            },
            kind: LayoutKind::NoticeLine,
            text_lines: vec![one_line(&card.line, text_width, LINE)],
        });
    }
}

/// Where a laid-out screen has the notice card, for a runtime that has to
/// repaint exactly that rectangle when it goes. `None` with no card.
#[must_use]
pub fn notice_rect(layout: &Layout) -> Option<Rect> {
    layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Notice)
        .map(|node| node.rect)
}
