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
    Sum {
        across: u8,
        down: u8,
    },
    Digit {
        value: u8,
        given: bool,
    },
    /// Pencil-mark candidates for one square, bits 0..=8 for digits 1..=9.
    Candidates(u16),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PencilMark {
    pub column: u8,
    pub row: u8,
    pub kind: PencilMarkKind,
    pub action: Option<ActionId>,
    pub selected: bool,
    /// A square sharing a row, column or box with the selected one.
    pub peer: bool,
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
                        PencilMarkKind::Candidates(m) => m.count_ones() as usize,
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
                || (mark.selected || mark.peer) && mark.action.is_none()
                || mark.selected && mark.peer
            {
                return false;
            }
            match mark.kind {
                PencilMarkKind::Clue(n) if n > 4 => return false,
                PencilMarkKind::Island(n) if !(1..=8).contains(&n) => return false,
                PencilMarkKind::Sum { across, down }
                    if across > 45 || down > 45 || across == 0 && down == 0 =>
                {
                    return false;
                }
                PencilMarkKind::Digit { value, .. } if value > 9 => return false,
                PencilMarkKind::Candidates(m) if m == 0 || m & !0x1FF != 0 => return false,
                _ => {}
            }
            if mark.action.is_some()
                && !matches!(
                    mark.kind,
                    PencilMarkKind::Island(_)
                        | PencilMarkKind::Digit { .. }
                        | PencilMarkKind::Candidates(_)
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
    // Physical geometry is the ceiling, not a demand: a board in a band slot
    // or beside a panel shrinks to its allocation rather than clipping.
    let cell = metrics
        .tenth_mm(i32::from(board.cell_tenth_mm))
        .min(area.width / i32::from(board.columns))
        .min(area.height / i32::from(board.rows));
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
    // A 9x9 board made only of digit squares is a sudoku: rule its 3x3 boxes
    // heavier so the structure a player scans by is visible.
    let boxes = board.columns == 9
        && board.rows == 9
        && board.marks.iter().all(|m| {
            matches!(
                m.kind,
                PencilMarkKind::Digit { .. } | PencilMarkKind::Candidates(_)
            )
        });
    let box_mask = |mark: &PencilMark| -> u8 {
        if !boxes {
            return 0;
        }
        let mut mask = 0;
        if mark.row % 3 == 0 {
            mask |= 1;
        }
        if mark.column % 3 == 0 {
            mask |= 2;
        }
        if mark.row == board.rows - 1 {
            mask |= 4;
        }
        if mark.column == board.columns - 1 {
            mask |= 8;
        }
        mask
    };
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
                        PencilMarkKind::Candidates(_) => "notes".into(),
                        _ => "clue".into(),
                    }
                )],
            );
        }
        push(
            layout,
            id,
            rect,
            LayoutKind::PencilMark(mark.kind, mark.selected, mark.peer, box_mask(mark)),
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
            PencilMarkKind::Digit { value, .. } if value != 0 => label(value, rect, mark.selected),
            PencilMarkKind::Candidates(mask) => {
                // Candidate digits get their whole ninth of the square: the
                // usual label padding would leave nothing at this size.
                let third = rect.width / 3;
                for d in 0..9_u8 {
                    if mask & (1 << d) != 0 {
                        push(
                            layout,
                            id,
                            Rect {
                                x: rect.x + i32::from(d % 3) * third,
                                y: rect.y + i32::from(d / 3) * third,
                                width: third,
                                height: third,
                            },
                            LayoutKind::PencilNumber(mark.selected),
                            vec![(d + 1).to_string()],
                        );
                    }
                }
            }
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
    for size in [FontSize::Body, FontSize::Caption, FontSize::Terminal] {
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
        if super::with_text_scale(scale, || fits(FontSize::Terminal)) {
            return (FontSize::Terminal, scale);
        }
    }
    // Nothing fit exactly: a single short label drawn dense at the smallest
    // scale keeps its glyph inside the box even when its leading does not.
    if node
        .text_lines
        .first()
        .is_some_and(|text| text.chars().count() <= 2)
        && super::with_text_scale(super::TextScale::STEPS[0], || {
            measure_text(&node.text_lines[0], FontSize::Terminal).0 <= node.rect.width
        })
    {
        return (FontSize::Terminal, super::TextScale::STEPS[0]);
    }
    (FontSize::Caption, current)
}
/// How a pencil mark presents: selection, peer shading, and the 3x3 box
/// rule mask for 9x9 digit boards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MarkStyle {
    pub selected: bool,
    pub peer: bool,
    pub box_mask: u8,
}

pub(super) fn draw_mark(
    surface: &mut Surface,
    rect: Rect,
    kind: PencilMarkKind,
    style: MarkStyle,
    metrics: &DisplayMetrics,
    clip: Rect,
) {
    let MarkStyle {
        selected,
        peer,
        box_mask,
    } = style;
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
        PencilMarkKind::Digit { .. } | PencilMarkKind::Candidates(_) => {
            fill_clipped(
                surface,
                rect,
                if selected {
                    tone::INK
                } else if peer {
                    tone::SURFACE
                } else {
                    tone::PAPER
                },
                clip,
            );
            stroke_clipped(surface, rect, tone::INK, line, clip);
            let heavy = line * 2;
            for (bit, side) in [
                (
                    1,
                    Rect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: heavy,
                    },
                ),
                (
                    2,
                    Rect {
                        x: rect.x,
                        y: rect.y,
                        width: heavy,
                        height: rect.height,
                    },
                ),
                (
                    4,
                    Rect {
                        x: rect.x,
                        y: rect.y + rect.height - heavy,
                        width: rect.width,
                        height: heavy,
                    },
                ),
                (
                    8,
                    Rect {
                        x: rect.x + rect.width - heavy,
                        y: rect.y,
                        width: heavy,
                        height: rect.height,
                    },
                ),
            ] {
                if box_mask & bit != 0 {
                    fill_clipped(surface, side, tone::INK, clip);
                }
            }
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
                    peer: false,
                },
                PencilMark {
                    column: 2,
                    row: 0,
                    kind: PencilMarkKind::Dot,
                    action: None,
                    selected: false,
                    peer: false,
                },
                PencilMark {
                    column: 2,
                    row: 2,
                    kind: PencilMarkKind::Dot,
                    action: None,
                    selected: false,
                    peer: false,
                },
                PencilMark {
                    column: 1,
                    row: 1,
                    kind: PencilMarkKind::Clue(2),
                    action: None,
                    selected: false,
                    peer: false,
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

    fn sudoku() -> PencilBoard {
        PencilBoard {
            columns: 9,
            rows: 9,
            cell_tenth_mm: 90,
            marks: vec![
                PencilMark {
                    column: 0,
                    row: 0,
                    kind: PencilMarkKind::Digit {
                        value: 5,
                        given: true,
                    },
                    action: None,
                    selected: false,
                    peer: false,
                },
                PencilMark {
                    column: 1,
                    row: 0,
                    kind: PencilMarkKind::Candidates(0b0000_0001_1000_0110),
                    action: Some(ActionId(20)),
                    selected: true,
                    peer: false,
                },
                PencilMark {
                    column: 3,
                    row: 4,
                    kind: PencilMarkKind::Digit {
                        value: 0,
                        given: false,
                    },
                    action: Some(ActionId(21)),
                    selected: false,
                    peer: true,
                },
            ],
            edges: vec![],
        }
    }
    #[test]
    fn sudoku_boards_rule_boxes_and_expand_candidates() {
        let board = sudoku();
        assert!(board.is_valid());
        let screen = Screen::new(
            1,
            vec![Node::PencilBoard {
                id: NodeId(1),
                board,
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let marks = layout
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, LayoutKind::PencilMark(..)))
            .collect::<Vec<_>>();
        assert_eq!(marks.len(), 3);
        assert!(matches!(
            marks[0].kind,
            LayoutKind::PencilMark(_, false, false, 0b0011)
        ));
        assert!(matches!(
            marks[1].kind,
            LayoutKind::PencilMark(_, true, false, 0b0001)
        ));
        assert!(matches!(
            marks[2].kind,
            LayoutKind::PencilMark(_, false, true, 0b0010)
        ));
        // The selected candidates square shows its four digits and no big number.
        let digits = layout
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, LayoutKind::PencilNumber(true)))
            .map(|n| n.text_lines[0].as_str())
            .collect::<Vec<_>>();
        assert_eq!(digits, ["2", "3", "8", "9"]);
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
    #[test]
    fn candidates_and_peers_are_bounded() {
        let mut b = sudoku();
        b.marks[1].kind = PencilMarkKind::Candidates(0);
        assert!(!b.is_valid());
        b = sudoku();
        b.marks[1].kind = PencilMarkKind::Candidates(1 << 9);
        assert!(!b.is_valid());
        b = sudoku();
        b.marks[1].peer = true;
        assert!(!b.is_valid());
        b = sudoku();
        b.marks[2].action = None;
        assert!(!b.is_valid());
        b = sudoku();
        b.marks[0].kind = PencilMarkKind::Clue(3);
        b.marks[0].action = Some(ActionId(22));
        assert!(!b.is_valid());
        // A 9x9 board with a non-digit mark is not ruled into boxes.
        b = sudoku();
        b.marks[2].kind = PencilMarkKind::Block;
        b.marks[2].action = None;
        b.marks[2].peer = false;
        assert!(b.is_valid());
        let screen = Screen::new(
            1,
            vec![Node::PencilBoard {
                id: NodeId(1),
                board: b,
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout
            .nodes
            .iter()
            .all(|n| !matches!(n.kind, LayoutKind::PencilMark(_, _, _, mask) if mask != 0)));
    }
}
