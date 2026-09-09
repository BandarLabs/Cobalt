//! Bounded geometry for pencil puzzles. Coordinates identify board positions,
//! while physical dimensions determine both ink and touch targets.
use super::{
    draw_vector, fill_clipped, fill_rounded_clipped, measure_text, stroke_clipped,
    stroke_rounded_clipped, tone, vector, ActionId, CellStyle, DisplayMetrics, FontSize, Glyph,
    Layout, LayoutKind, LayoutNode, NodeId, Rect, Surface,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PencilMarkKind {
    Dot,
    Clue(u8),
    Island(u8),
    Block,
    Sum { across: u8, down: u8 },
    Digit { value: u8, given: bool },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PencilMark {
    pub column: u8,
    pub row: u8,
    pub kind: PencilMarkKind,
    pub action: Option<ActionId>,
    pub selected: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PencilEdge {
    pub from: (u8, u8),
    pub to: (u8, u8),
    /// 0 blank, 1 single, 2 double, 3 excluded.
    pub state: u8,
    pub action: Option<ActionId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PencilBoard {
    pub columns: u8,
    pub rows: u8,
    pub cell_tenth_mm: u16,
    pub marks: Vec<PencilMark>,
    pub edges: Vec<PencilEdge>,
}
impl PencilBoard {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        if !(1..=9).contains(&self.columns)
            || !(1..=9).contains(&self.rows)
            || !(80..=300).contains(&self.cell_tenth_mm)
            || self.marks.is_empty()
            || self.marks.len() > 81
            || self.edges.len() > 144
        {
            return false;
        }
        let nodes = 1
            + self.action_count()
            + self.edges.len()
            + self
                .marks
                .iter()
                .map(|m| {
                    1 + match m.kind {
                        PencilMarkKind::Sum { across, down } => {
                            usize::from(across != 0) + usize::from(down != 0)
                        }
                        PencilMarkKind::Clue(_) | PencilMarkKind::Island(_) => 1,
                        PencilMarkKind::Digit { value, .. } => usize::from(value != 0),
                        _ => 0,
                    }
                })
                .sum::<usize>();
        if nodes > super::MAX_LAYOUT_NODES {
            return false;
        }
        let mut positions = std::collections::BTreeMap::new();
        let mut actions = std::collections::BTreeSet::new();
        let mut action_ok = |action: Option<ActionId>| {
            action.is_none_or(|id| !id.is_reserved() && actions.insert(id))
        };
        for mark in &self.marks {
            if mark.column >= self.columns
                || mark.row >= self.rows
                || positions
                    .insert((mark.column, mark.row), mark.kind)
                    .is_some()
                || !action_ok(mark.action)
                || mark.selected && mark.action.is_none()
            {
                return false;
            }
            match mark.kind {
                PencilMarkKind::Clue(n) if n > 4 => return false,
                PencilMarkKind::Island(n) if !(1..=8).contains(&n) => return false,
                PencilMarkKind::Sum { across, down }
                    if across > 45 || down > 45 || across == 0 && down == 0 =>
                {
                    return false
                }
                PencilMarkKind::Digit { value, .. } if value > 9 => return false,
                _ => {}
            }
            if mark.action.is_some()
                && !matches!(
                    mark.kind,
                    PencilMarkKind::Island(_) | PencilMarkKind::Digit { given: false, .. }
                )
            {
                return false;
            }
        }
        let mut edges = std::collections::BTreeSet::new();
        for edge in &self.edges {
            let vertical = edge.from.0 == edge.to.0;
            let horizontal = edge.from.1 == edge.to.1;
            let distance = if vertical {
                edge.from.1.abs_diff(edge.to.1)
            } else {
                edge.from.0.abs_diff(edge.to.0)
            };
            if vertical == horizontal
                || distance < 2
                || edge.state > 3
                || !action_ok(edge.action)
                || !edges.insert((edge.from.min(edge.to), edge.from.max(edge.to)))
            {
                return false;
            }
            for end in [edge.from, edge.to] {
                if !matches!(
                    positions.get(&end),
                    Some(PencilMarkKind::Dot | PencilMarkKind::Island(_))
                ) {
                    return false;
                }
            }
        }
        true
    }
    #[must_use]
    pub fn action_count(&self) -> usize {
        self.marks.iter().filter(|m| m.action.is_some()).count()
            + self.edges.iter().filter(|e| e.action.is_some()).count()
    }
}

fn push(layout: &mut Layout, id: NodeId, rect: Rect, kind: LayoutKind, text_lines: Vec<String>) {
    if layout.nodes.len() >= super::MAX_LAYOUT_NODES {
        return;
    }
    layout.nodes.push(LayoutNode {
        id,
        rect,
        kind,
        text_lines,
    });
}
pub(super) fn layout(
    id: NodeId,
    board: &PencilBoard,
    area: Rect,
    metrics: &DisplayMetrics,
    layout: &mut Layout,
) -> i32 {
    if !board.is_valid() {
        return area.y;
    }
    let cell = metrics.tenth_mm(i32::from(board.cell_tenth_mm));
    let width = i32::from(board.columns) * cell;
    let height = i32::from(board.rows) * cell;
    let left = area.x + (area.width - width).max(0) / 2;
    let rect = Rect {
        x: left,
        y: area.y,
        width,
        height,
    };
    push(layout, id, rect, LayoutKind::Spacer, vec![]);
    for edge in &board.edges {
        let vertical = edge.from.0 == edge.to.0;
        let x = left + i32::from(edge.from.0.min(edge.to.0)) * cell + cell / 2;
        let y = area.y + i32::from(edge.from.1.min(edge.to.1)) * cell + cell / 2;
        let length = i32::from(if vertical {
            edge.from.1.abs_diff(edge.to.1)
        } else {
            edge.from.0.abs_diff(edge.to.0)
        }) * cell;
        let ink = if vertical {
            Rect {
                x: x - cell / 2,
                y,
                width: cell,
                height: length,
            }
        } else {
            Rect {
                x,
                y: y - cell / 2,
                width: length,
                height: cell,
            }
        };
        let hit = if vertical {
            Rect {
                y: y + cell / 2,
                height: length - cell,
                ..ink
            }
        } else {
            Rect {
                x: x + cell / 2,
                width: length - cell,
                ..ink
            }
        };
        if let Some(action) = edge.action {
            push(
                layout,
                id,
                hit,
                LayoutKind::Cell(action, CellStyle::Plain, false),
                vec![format!(
                    "{} {},{} to {},{}: {}",
                    if vertical { "Vertical" } else { "Horizontal" },
                    edge.from.1 + 1,
                    edge.from.0 + 1,
                    edge.to.1 + 1,
                    edge.to.0 + 1,
                    match edge.state {
                        0 => "blank",
                        1 => "single",
                        2 => "double",
                        _ => "excluded",
                    }
                )],
            );
        }
        push(
            layout,
            id,
            ink,
            LayoutKind::PencilEdge(edge.state, vertical),
            vec![],
        );
    }
    for mark in &board.marks {
        let rect = Rect {
            x: left + i32::from(mark.column) * cell,
            y: area.y + i32::from(mark.row) * cell,
            width: cell,
            height: cell,
        };
        if let Some(action) = mark.action {
            push(
                layout,
                id,
                rect,
                LayoutKind::Cell(action, CellStyle::Plain, mark.selected),
                vec![format!(
                    "Row {}, column {}, {}",
                    mark.row + 1,
                    mark.column + 1,
                    match mark.kind {
                        PencilMarkKind::Island(n) => format!("island {n}"),
                        PencilMarkKind::Digit { value: 0, .. } => "blank square".into(),
                        PencilMarkKind::Digit { value, .. } => format!("digit {value}"),
                        _ => "clue".into(),
                    }
                )],
            );
        }
        push(
            layout,
            id,
            rect,
            LayoutKind::PencilMark(mark.kind, mark.selected),
            vec![],
        );
        let mut label = |value: u8, area: Rect, inverted: bool| {
            let pad = metrics.rule_thickness() * 2;
            let area = Rect {
                x: area.x + pad,
                y: area.y + pad,
                width: area.width - pad * 2,
                height: area.height - pad * 2,
            };
            push(
                layout,
                id,
                area,
                LayoutKind::PencilNumber(inverted),
                vec![value.to_string()],
            );
        };
        match mark.kind {
            PencilMarkKind::Clue(n) | PencilMarkKind::Island(n) => label(n, rect, false),
            PencilMarkKind::Digit { value, .. } if value != 0 => label(value, rect, false),
            PencilMarkKind::Sum { across, down } => {
                let half = rect.width / 2;
                if across != 0 {
                    label(
                        across,
                        Rect {
                            x: rect.x + half,
                            y: rect.y,
                            width: half,
                            height: half,
                        },
                        true,
                    );
                }
                if down != 0 {
                    label(
                        down,
                        Rect {
                            x: rect.x,
                            y: rect.y + half,
                            width: half,
                            height: half,
                        },
                        true,
                    );
                }
            }
            _ => {}
        }
    }
    area.y + height
}
pub(super) fn label_style(node: &LayoutNode) -> (FontSize, super::TextScale) {
    let current = super::text_scale();
    let fits = |size| {
        node.text_lines
            .iter()
            .all(|text| measure_text(text, size).0 <= node.rect.width)
            && size.line_height() <= node.rect.height
    };
    for size in [FontSize::Body, FontSize::Caption] {
        if fits(size) {
            return (size, current);
        }
    }
    for scale in super::TextScale::STEPS
        .into_iter()
        .take_while(|s| *s != current)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        if super::with_text_scale(scale, || fits(FontSize::Caption)) {
            return (FontSize::Caption, scale);
        }
    }
    (FontSize::Caption, current)
}
pub(super) fn draw_mark(
    surface: &mut Surface,
    rect: Rect,
    kind: PencilMarkKind,
    selected: bool,
    metrics: &DisplayMetrics,
    clip: Rect,
) {
    let line = metrics.rule_thickness().max(1);
    match kind {
        PencilMarkKind::Dot => {
            let side = (line * 3).max(4);
            let dot = Rect {
                x: rect.x + (rect.width - side) / 2,
                y: rect.y + (rect.height - side) / 2,
                width: side,
                height: side,
            };
            fill_rounded_clipped(surface, dot, side / 2, tone::INK, clip);
        }
        PencilMarkKind::Clue(_) => {}
        PencilMarkKind::Island(_) => {
            let margin = rect.width / 8;
            let circle = Rect {
                x: rect.x + margin,
                y: rect.y + margin,
                width: rect.width - margin * 2,
                height: rect.height - margin * 2,
            };
            fill_rounded_clipped(surface, circle, circle.width / 2, tone::PAPER, clip);
            stroke_rounded_clipped(surface, circle, circle.width / 2, tone::INK, line, clip);
        }
        PencilMarkKind::Block => fill_clipped(surface, rect, tone::INK, clip),
        PencilMarkKind::Digit { .. } => {
            fill_clipped(
                surface,
                rect,
                if selected { tone::SURFACE } else { tone::PAPER },
                clip,
            );
            stroke_clipped(surface, rect, tone::INK, line, clip);
        }
        PencilMarkKind::Sum { .. } => {
            fill_clipped(surface, rect, tone::INK, clip);
            // The diagonal separates the down clue at lower left from across at upper right.
            for n in 0..rect.width {
                fill_clipped(
                    surface,
                    Rect {
                        x: rect.x + n,
                        y: rect.y + n * rect.height / rect.width,
                        width: line,
                        height: line,
                    },
                    tone::PAPER,
                    clip,
                );
            }
        }
    }
}
pub(super) fn draw_edge(
    surface: &mut Surface,
    rect: Rect,
    state: u8,
    vertical: bool,
    metrics: &DisplayMetrics,
    clip: Rect,
) {
    if state == 0 {
        return;
    }
    let thickness = metrics.rule_thickness().max(1) * 2;
    if state == 3 {
        let side = if vertical { rect.width } else { rect.height } / 3;
        draw_vector(
            surface,
            &vector::shapes(Glyph::Close),
            Rect {
                x: rect.x + (rect.width - side) / 2,
                y: rect.y + (rect.height - side) / 2,
                width: side,
                height: side,
            },
            clip,
            tone::MUTED,
        );
        return;
    }
    for offset in if state == 2 {
        vec![-thickness, thickness]
    } else {
        vec![0]
    } {
        let line = if vertical {
            Rect {
                x: rect.x + (rect.width - thickness) / 2 + offset,
                width: thickness,
                ..rect
            }
        } else {
            Rect {
                y: rect.y + (rect.height - thickness) / 2 + offset,
                height: thickness,
                ..rect
            }
        };
        fill_clipped(surface, line, tone::INK, clip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chrome, Node, Screen, CLARA_BW_METRICS};
    fn sample() -> PencilBoard {
        PencilBoard {
            columns: 3,
            rows: 3,
            cell_tenth_mm: 100,
            marks: vec![
                PencilMark {
                    column: 0,
                    row: 0,
                    kind: PencilMarkKind::Dot,
                    action: None,
                    selected: false,
                },
                PencilMark {
                    column: 2,
                    row: 0,
                    kind: PencilMarkKind::Dot,
                    action: None,
                    selected: false,
                },
                PencilMark {
                    column: 2,
                    row: 2,
                    kind: PencilMarkKind::Dot,
                    action: None,
                    selected: false,
                },
                PencilMark {
                    column: 1,
                    row: 1,
                    kind: PencilMarkKind::Clue(2),
                    action: None,
                    selected: false,
                },
            ],
            edges: vec![
                PencilEdge {
                    from: (0, 0),
                    to: (2, 0),
                    state: 1,
                    action: Some(ActionId(10)),
                },
                PencilEdge {
                    from: (2, 0),
                    to: (2, 2),
                    state: 1,
                    action: Some(ActionId(11)),
                },
            ],
        }
    }
    #[test]
    fn clues_are_fixed_and_edges_have_distinct_physical_targets() {
        let board = sample();
        assert!(board.is_valid());
        let screen = Screen::new(
            1,
            vec![Node::PencilBoard {
                id: NodeId(1),
                board,
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let targets = layout
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, LayoutKind::Cell(..)))
            .collect::<Vec<_>>();
        assert_eq!(targets.len(), 2);
        assert!(targets
            .iter()
            .all(|n| n.rect.width >= CLARA_BW_METRICS.touch_target_minimum()
                && n.rect.height >= CLARA_BW_METRICS.touch_target_minimum()));
        assert!(targets[0].rect.intersection(targets[1].rect).is_none());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
    #[test]
    fn continuous_edges_and_double_bridges_use_ink_geometry_and_obey_dirty_clip() {
        let metrics = CLARA_BW_METRICS;
        let rect = Rect {
            x: 20,
            y: 20,
            width: 80,
            height: 20,
        };
        let mut surface = Surface::new(120, 80);
        surface.pixels.fill(tone::PAPER);
        draw_edge(
            &mut surface,
            rect,
            1,
            false,
            &metrics,
            Rect {
                x: 0,
                y: 0,
                width: 120,
                height: 80,
            },
        );
        for x in 20..100 {
            assert_eq!(surface.pixels[30 * 120 + x], tone::INK);
        }
        surface.pixels.fill(tone::PAPER);
        let clip = Rect {
            x: 40,
            y: 20,
            width: 20,
            height: 20,
        };
        draw_edge(&mut surface, rect, 2, false, &metrics, clip);
        for y in 0..80 {
            for x in 0..120 {
                if !clip.contains(x, y) {
                    assert_eq!(surface.pixels[(y * 120 + x) as usize], tone::PAPER);
                }
            }
        }
        assert!(surface.pixels.contains(&tone::INK));
    }
    #[test]
    fn ambiguous_or_out_of_bounds_geometry_is_invalid() {
        let mut b = sample();
        b.edges.push(b.edges[0].clone());
        assert!(!b.is_valid());
        b = sample();
        b.marks[0].column = 9;
        assert!(!b.is_valid());
        b = sample();
        b.edges[0].from = (1, 1);
        assert!(!b.is_valid());
        b = sample();
        b.marks[3].action = Some(ActionId(19));
        assert!(!b.is_valid());
        b = sample();
        b.edges[1].action = b.edges[0].action;
        assert!(!b.is_valid());
    }
}
