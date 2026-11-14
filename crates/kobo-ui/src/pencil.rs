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
        let mut mask = RULED_BOXES;
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
        let mut label = |value: u8, area: Rect, inverted: bool, entered: bool| {
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
                if entered {
                    LayoutKind::PencilEntry(inverted)
                } else {
                    LayoutKind::PencilNumber(inverted)
                },
                vec![value.to_string()],
            );
        };
        match mark.kind {
            PencilMarkKind::Clue(n) | PencilMarkKind::Island(n) => label(n, rect, false, false),
            PencilMarkKind::Digit { value, given } if value != 0 => {
                label(value, rect, mark.selected, !given);
            }
            PencilMarkKind::Candidates(mask) => {
                // Candidate digits get a ninth of the square each, in the
                // place a solver pencils them: 1 top left, 9 bottom right.
                // The ninths are taken from a square a little inside the
                // rules, so a note does not sit on the line it is next to.
                let inset = rect.width / 12;
                let third = (rect.width - inset * 2) / 3;
                for d in 0..9_u8 {
                    if mask & (1 << d) != 0 {
                        push(
                            layout,
                            id,
                            Rect {
                                x: rect.x + inset + i32::from(d % 3) * third,
                                y: rect.y + inset + i32::from(d / 3) * third,
                                width: third,
                                height: third,
                            },
                            LayoutKind::PencilNote(mark.selected),
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
                        false,
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
                        false,
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
    // A lone digit is its glyph, not its leading, so it may take a size
    // whose line is a hair taller than its box, as the layout check allows.
    // Without that a note in a ninth of a square fell three sizes to one a
    // reader had to lean in for.
    let allowance = if node.text_lines.len() == 1 {
        node.rect.height / 6
    } else {
        0
    };
    let fits = |size: FontSize| {
        node.text_lines
            .iter()
            .all(|text| measure_text(text, size).0 <= node.rect.width)
            && size.line_height() <= node.rect.height + allowance
    };
    // A digit in a square is set as large as a printed puzzle sets it.
    let largest: &[FontSize] = if node
        .text_lines
        .first()
        .is_some_and(|text| text.chars().count() == 1)
    {
        &[
            FontSize::Title,
            FontSize::Body,
            FontSize::Caption,
            FontSize::Terminal,
        ]
    } else {
        &[FontSize::Body, FontSize::Caption, FontSize::Terminal]
    };
    for size in largest.iter().copied() {
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
/// Set on every square of a board ruled into 3x3 boxes, beside the bits for
/// which of its sides are box rules: 1 top, 2 left, 4 bottom, 8 right.
pub(super) const RULED_BOXES: u8 = 16;

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
    palette: crate::Palette,
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
            // The selected cell is the one filled control of the board, and
            // takes the accent the way a primary button does; its digit is
            // reversed out in paper either way. Peers stay on the quiet grey.
            fill_clipped(
                surface,
                rect,
                if selected {
                    palette.emphasis()
                } else if peer {
                    crate::Paint::Grey(tone::SURFACE)
                } else {
                    crate::Paint::Grey(tone::PAPER)
                },
                clip,
            );
            // A board ruled into boxes is set the way a printed sudoku is:
            // a hairline between squares and a clearly heavier rule round
            // each box. Each square drew its own full outline before, so the
            // rule between two squares came out twice the outline, and the
            // box rule on top of that was barely heavier than its neighbours.
            // A square now draws only its top and left hairlines, which its
            // neighbours complete, and the box rules fall on the sides the
            // mask names.
            let ruled = box_mask & RULED_BOXES != 0;
            let heavy = if ruled {
                let hair = metrics.tenth_mm(1).max(1);
                for side in [
                    Rect {
                        height: hair,
                        ..rect
                    },
                    Rect {
                        width: hair,
                        ..rect
                    },
                ] {
                    fill_clipped(surface, side, tone::INK, clip);
                }
                metrics.tenth_mm(5).max(hair * 3)
            } else {
                stroke_clipped(surface, rect, tone::INK, line, clip);
                line * 2
            };
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
            LayoutKind::PencilMark(_, false, false, 0b1_0011)
        ));
        assert!(matches!(
            marks[1].kind,
            LayoutKind::PencilMark(_, true, false, 0b1_0001)
        ));
        assert!(matches!(
            marks[2].kind,
            LayoutKind::PencilMark(_, false, true, 0b1_0010)
        ));
        // The selected candidates square shows its four notes, each in its
        // own ninth of the square, and no big number.
        let notes = layout
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, LayoutKind::PencilNote(true)))
            .collect::<Vec<_>>();
        let digits = notes
            .iter()
            .map(|n| n.text_lines[0].as_str())
            .collect::<Vec<_>>();
        assert_eq!(digits, ["2", "3", "8", "9"]);
        let square = marks[1].rect;
        let place = |note: &LayoutNode| {
            (
                (note.rect.x + note.rect.width / 2 - square.x) * 3 / square.width,
                (note.rect.y + note.rect.height / 2 - square.y) * 3 / square.height,
            )
        };
        assert_eq!(
            notes.iter().map(|note| place(note)).collect::<Vec<_>>(),
            [(1, 0), (2, 0), (1, 2), (2, 2)]
        );
        assert!(layout
            .nodes
            .iter()
            .all(|n| !matches!(n.kind, LayoutKind::PencilNumber(true))));
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
    #[test]
    fn an_entered_digit_is_grey_beside_the_printed_ones() {
        let mut board = sudoku();
        board.marks[2].kind = PencilMarkKind::Digit {
            value: 4,
            given: false,
        };
        let screen = Screen::new(
            1,
            vec![Node::PencilBoard {
                id: NodeId(1),
                board,
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let kinds = layout
            .nodes
            .iter()
            .filter_map(|n| match n.kind {
                LayoutKind::PencilNumber(_) | LayoutKind::PencilEntry(_) => {
                    Some((n.text_lines[0].clone(), n.kind))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                ("5".to_owned(), LayoutKind::PencilNumber(false)),
                ("4".to_owned(), LayoutKind::PencilEntry(false)),
            ]
        );
        let mut surface = crate::Surface::new(
            CLARA_BW_METRICS.width as usize,
            CLARA_BW_METRICS.height as usize,
        );
        crate::render_with(
            &screen,
            &CLARA_BW_METRICS,
            &Chrome::default(),
            &mut surface,
            None,
        );
        let darkest = |kind: LayoutKind| {
            let rect = layout.nodes.iter().find(|n| n.kind == kind).unwrap().rect;
            (rect.y..rect.y + rect.height)
                .flat_map(|y| (rect.x..rect.x + rect.width).map(move |x| (x, y)))
                .map(|(x, y)| surface.pixels[y as usize * surface.width + x as usize])
                .min()
                .unwrap()
        };
        assert_eq!(darkest(LayoutKind::PencilNumber(false)), tone::INK);
        assert!(darkest(LayoutKind::PencilEntry(false)) >= tone::MUTED - 8);
    }
    /// The widths of the runs of ink along one row of pixels, left to right.
    fn ink_runs(surface: &crate::Surface, y: usize, from: usize, to: usize) -> Vec<usize> {
        let mut runs = Vec::new();
        let mut run = 0;
        for x in from..to {
            if surface.pixels[y * surface.width + x] < 0x40 {
                run += 1;
            } else if run > 0 {
                runs.push(run);
                run = 0;
            }
        }
        if run > 0 {
            runs.push(run);
        }
        runs
    }
    #[test]
    fn a_sudoku_is_ruled_with_hairlines_between_squares_and_heavy_rules_round_boxes() {
        let board = PencilBoard {
            columns: 9,
            rows: 9,
            cell_tenth_mm: 80,
            marks: (0..81u8)
                .map(|cell| PencilMark {
                    column: cell % 9,
                    row: cell / 9,
                    kind: PencilMarkKind::Digit {
                        value: 0,
                        given: false,
                    },
                    action: Some(ActionId(100 + u32::from(cell))),
                    selected: false,
                    peer: false,
                })
                .collect(),
            edges: vec![],
        };
        let screen = Screen::new(
            1,
            vec![Node::PencilBoard {
                id: NodeId(1),
                board,
            }],
        );
        let metrics = CLARA_BW_METRICS;
        let layout = screen.layout_with(&metrics, &Chrome::default());
        let squares = layout
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, LayoutKind::PencilMark(..)))
            .map(|n| n.rect)
            .collect::<Vec<_>>();
        let (first, last) = (squares[0], squares[80]);
        let mut surface = crate::Surface::new(metrics.width as usize, metrics.height as usize);
        crate::render_with(&screen, &metrics, &Chrome::default(), &mut surface, None);
        // Through the middle of the fifth row, from just outside the board to
        // just past it: ten rules, the four box rules at either end and every
        // third, and hairlines between.
        let y = usize::try_from(squares[36].y + squares[36].height / 2).unwrap();
        let runs = ink_runs(
            &surface,
            y,
            usize::try_from(first.x - 4).unwrap(),
            usize::try_from(last.x + last.width + 4).unwrap(),
        );
        assert_eq!(runs.len(), 10, "{runs:?}");
        let (boxes, hairlines): (Vec<_>, Vec<_>) = runs
            .iter()
            .enumerate()
            .partition(|(index, _)| index % 3 == 0);
        let thinnest_box = boxes.iter().map(|(_, run)| **run).min().unwrap();
        let thickest_hairline = hairlines.iter().map(|(_, run)| **run).max().unwrap();
        assert!(thickest_hairline <= 2, "{runs:?}");
        assert!(thinnest_box >= thickest_hairline * 3, "{runs:?}");
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
