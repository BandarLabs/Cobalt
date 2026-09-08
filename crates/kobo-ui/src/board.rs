//! Board geometry is explicit: font scaling never changes puzzle coordinates.
use super::{
    draw_centered, draw_vector, fill_clipped, fill_rounded_clipped, measure_text, tone, vector,
    ActionId, CellStyle, DisplayMetrics, FontSize, Glyph, Layout, LayoutKind, LayoutNode, NodeId,
    Rect, Space, Surface, MAX_LAYOUT_NODES,
};

pub const MAX_VISIBLE: usize = 81;
pub const MAX_CLUES: usize = 32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BoardMark {
    #[default]
    Empty,
    Filled,
    Crossed,
    Dot,
    Value(u16),
    Notes(u32),
}
impl BoardMark {
    #[must_use]
    pub const fn is_valid(self) -> bool {
        !matches!(self, Self::Value(0) | Self::Notes(0))
    }
    #[must_use]
    pub fn description(self) -> String {
        match self {
            Self::Empty => "empty".into(),
            Self::Filled => "filled".into(),
            Self::Crossed => "crossed out".into(),
            Self::Dot => "dot".into(),
            Self::Value(value) => value.to_string(),
            Self::Notes(bits) => format!(
                "notes {}",
                (0..32)
                    .filter(|bit| bits & (1 << bit) != 0)
                    .map(|bit| (bit + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardCell {
    /// Selecting a given is allowed for inspection; the app prevents editing it.
    pub action: ActionId,
    pub mark: BoardMark,
    pub given: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardClue {
    /// Opens the complete clue. A long gutter clue visibly ends in an ellipsis.
    pub action: ActionId,
    pub values: Vec<u8>,
}

/// A window into a board of at most 64 × 64. All indices remain absolute.
/// Clues are empty or match the visible row/column count. Zero values are not
/// clues: use an empty list for an empty line. The app handles clue inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoardSurface {
    pub columns: u8,
    pub row_start: u8,
    pub column_start: u8,
    pub cell_tenth_mm: u16,
    pub cells: Vec<BoardCell>,
    pub row_clues: Vec<BoardClue>,
    pub column_clues: Vec<BoardClue>,
}
impl BoardSurface {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let columns = usize::from(self.columns);
        if !(1..=12).contains(&columns)
            || self.cells.is_empty()
            || self.cells.len() > MAX_VISIBLE
            || self.cells.len() % columns != 0
            || !(60..=300).contains(&self.cell_tenth_mm)
        {
            return false;
        }
        let rows = self.cells.len() / columns;
        if rows > 12
            || usize::from(self.row_start) + rows > 64
            || usize::from(self.column_start) + columns > 64
            || (!self.row_clues.is_empty() && self.row_clues.len() != rows)
            || (!self.column_clues.is_empty() && self.column_clues.len() != columns)
        {
            return false;
        }
        let mut actions = std::collections::BTreeSet::new();
        self.cells.iter().all(|cell| {
            cell.mark.is_valid() && !cell.action.is_reserved() && actions.insert(cell.action)
        }) && self.row_clues.iter().chain(&self.column_clues).all(|clue| {
            !clue.action.is_reserved()
                && actions.insert(clue.action)
                && clue.values.len() <= MAX_CLUES
                && clue.values.iter().all(|value| (1..=64).contains(value))
        })
    }

    /// Gutter dimensions used by both SDK viewport fitting and layout.
    #[must_use]
    pub fn gutters(metrics: &DisplayMetrics, row_clues: bool, column_clues: bool) -> (i32, i32) {
        crate::with_text_scale(metrics.text_scale, || {
            let padding = metrics.space(Space::Tight) * 2;
            (
                if row_clues {
                    metrics
                        .touch_target_default()
                        .max(measure_text("64 …", FontSize::Caption).0 + padding)
                } else {
                    0
                },
                if column_clues {
                    metrics
                        .touch_target_default()
                        .max(FontSize::Caption.line_height() * 3 + padding)
                } else {
                    0
                },
            )
        })
    }
}

pub(super) fn layout(
    id: NodeId,
    board: &BoardSurface,
    area: Rect,
    metrics: &DisplayMetrics,
    layout: &mut Layout,
) -> i32 {
    if !board.is_valid() {
        return area.y;
    }
    let columns = i32::from(board.columns);
    let rows = board.cells.len() as i32 / columns;
    let cell = metrics.tenth_mm(i32::from(board.cell_tenth_mm));
    let gap = metrics.rule_thickness().max(1);
    let (left, top) = BoardSurface::gutters(
        metrics,
        !board.row_clues.is_empty(),
        !board.column_clues.is_empty(),
    );
    let width = left + columns * cell + (columns - 1) * gap;
    let height = top + rows * cell + (rows - 1) * gap;
    let origin = Rect {
        x: area.x + (area.width - width).max(0) / 2,
        y: area.y,
        width,
        height,
    };
    layout.nodes.push(LayoutNode {
        id,
        rect: origin,
        kind: LayoutKind::Spacer,
        text_lines: Vec::new(),
    });
    for (index, value) in board.cells.iter().enumerate() {
        if layout.nodes.len() + 2 > MAX_LAYOUT_NODES {
            break;
        }
        let row = index as i32 / columns;
        let column = index as i32 % columns;
        let rect = Rect {
            x: origin.x + left + column * (cell + gap),
            y: origin.y + top + row * (cell + gap),
            width: cell,
            height: cell,
        };
        let description = format!(
            "Row {}, column {}, {}{}",
            i32::from(board.row_start) + row + 1,
            i32::from(board.column_start) + column + 1,
            value.mark.description(),
            if value.given { ", given" } else { "" }
        );
        layout.nodes.push(LayoutNode {
            id,
            rect,
            kind: LayoutKind::Cell(value.action, CellStyle::Board, value.selected),
            text_lines: vec![description],
        });
        layout.nodes.push(LayoutNode {
            id,
            rect,
            kind: LayoutKind::BoardMark(value.mark, value.given),
            text_lines: Vec::new(),
        });
    }
    for (vertical, clues) in [(false, &board.row_clues), (true, &board.column_clues)] {
        for (index, clue) in clues.iter().enumerate() {
            if layout.nodes.len() + 2 > MAX_LAYOUT_NODES {
                break;
            }
            let rect = if vertical {
                Rect {
                    x: origin.x + left + index as i32 * (cell + gap),
                    y: origin.y,
                    width: cell,
                    height: top,
                }
            } else {
                Rect {
                    x: origin.x,
                    y: origin.y + top + index as i32 * (cell + gap),
                    width: left,
                    height: cell,
                }
            };
            let axis = if vertical { "Column" } else { "Row" };
            let start = if vertical {
                board.column_start
            } else {
                board.row_start
            };
            let all = if clue.values.is_empty() {
                "0".into()
            } else {
                clue.values
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            layout.nodes.push(LayoutNode {
                id,
                rect,
                kind: LayoutKind::Cell(clue.action, CellStyle::Plain, false),
                text_lines: vec![format!(
                    "{axis} {} clue: {all}",
                    usize::from(start) + index + 1
                )],
            });
            let lines = if clue.values.is_empty() {
                vec!["0".into()]
            } else if vertical {
                let mut lines = clue
                    .values
                    .iter()
                    .take(if clue.values.len() <= 3 { 3 } else { 2 })
                    .map(u8::to_string)
                    .collect::<Vec<_>>();
                if clue.values.len() > 3 {
                    lines.push("…".into());
                }
                lines
            } else if clue.values.len() == 1 {
                vec![all]
            } else {
                vec![format!("{} …", clue.values[0])]
            };
            layout.nodes.push(LayoutNode {
                id,
                rect,
                kind: LayoutKind::BoardClue,
                text_lines: lines,
            });
        }
    }
    origin.y + height
}

pub(super) fn draw_mark(
    surface: &mut Surface,
    rect: Rect,
    mark: BoardMark,
    given: bool,
    metrics: &DisplayMetrics,
    clip: Rect,
) {
    let Some(clip) = clip.intersection(rect) else {
        return;
    };
    let inset = (rect.width / 5).max(metrics.rule_thickness() * 3);
    let inner = Rect {
        x: rect.x + inset,
        y: rect.y + inset,
        width: (rect.width - inset * 2).max(0),
        height: (rect.height - inset * 2).max(0),
    };
    match mark {
        BoardMark::Empty => {}
        BoardMark::Filled => fill_clipped(surface, inner, tone::INK, clip),
        BoardMark::Crossed => draw_vector(
            surface,
            &vector::shapes(Glyph::Close),
            inner,
            clip,
            tone::INK,
        ),
        BoardMark::Dot => {
            let size = (inner.width / 4).max(2);
            fill_rounded_clipped(
                surface,
                Rect {
                    x: rect.x + (rect.width - size) / 2,
                    y: rect.y + (rect.height - size) / 2,
                    width: size,
                    height: size,
                },
                size / 2,
                tone::INK,
                clip,
            );
        }
        BoardMark::Value(value) => {
            let text = value.to_string();
            let size = if measure_text(&text, FontSize::Body).0 <= inner.width {
                FontSize::Body
            } else {
                FontSize::Caption
            };
            let text = if measure_text(&text, size).0 <= inner.width {
                text
            } else {
                "…".into()
            };
            draw_centered(surface, &[text], inner, size, tone::INK, clip);
        }
        BoardMark::Notes(bits) => draw_centered(
            surface,
            &[bits.count_ones().to_string(), "notes".into()],
            rect,
            FontSize::Caption,
            tone::INK,
            clip,
        ),
    }
    if given {
        let line = metrics.rule_thickness().max(1);
        fill_clipped(
            surface,
            Rect {
                x: inner.x,
                y: rect.y + rect.height - line * 3,
                width: inner.width,
                height: line,
            },
            tone::INK,
            clip,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{render_with, Cell, Chrome, Node, Screen, CLARA_BW_METRICS};
    #[test]
    fn a_three_number_column_clue_is_complete_and_longer_clues_show_ellipsis() {
        for count in 0..=4 {
            let surface = BoardSurface {
                columns: 1,
                row_start: 0,
                column_start: 0,
                cell_tenth_mm: 120,
                cells: vec![BoardCell {
                    action: ActionId(7),
                    mark: BoardMark::Empty,
                    given: false,
                    selected: false,
                }],
                row_clues: vec![],
                column_clues: vec![BoardClue {
                    action: ActionId(8),
                    values: (1..=count).collect(),
                }],
            };
            let screen = Screen::new(
                1,
                vec![Node::Board {
                    id: NodeId(1),
                    surface,
                }],
            );
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            let clue = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::BoardClue)
                .unwrap();
            let expected = match count {
                0 => vec!["0".to_owned()],
                1..=3 => (1..=count).map(|value| value.to_string()).collect(),
                _ => vec!["1".into(), "2".into(), "…".into()],
            };
            assert_eq!(clue.text_lines, expected);
        }
    }

    #[test]
    fn narrow_square_grids_keep_their_original_coordinates() {
        let metrics = DisplayMetrics {
            width: 400,
            height: 800,
            ..CLARA_BW_METRICS
        };
        let screen = Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(1),
                columns: 12,
                square: true,
                cells: (0..24)
                    .map(|cell| Cell::new(ActionId(cell + 1), ""))
                    .collect(),
            }],
        );
        let layout = screen.layout_with(&metrics, &Chrome::default());
        let first = layout.rect_of_action(ActionId(1)).unwrap();
        assert_eq!(layout.rect_of_action(ActionId(12)).unwrap().y, first.y);
        assert!(layout.rect_of_action(ActionId(13)).unwrap().y > first.y);
    }
    #[test]
    fn all_marks_selection_and_given_ink_are_distinct_and_confined() {
        let mut images = std::collections::BTreeSet::new();
        for mark in [
            BoardMark::Empty,
            BoardMark::Filled,
            BoardMark::Crossed,
            BoardMark::Dot,
            BoardMark::Value(7),
            BoardMark::Notes(5),
        ] {
            for given in [false, true] {
                for selected in [false, true] {
                    let screen = Screen::new(
                        1,
                        vec![Node::Board {
                            id: NodeId(1),
                            surface: BoardSurface {
                                columns: 1,
                                row_start: 0,
                                column_start: 0,
                                cell_tenth_mm: 120,
                                cells: vec![BoardCell {
                                    action: ActionId(7),
                                    mark,
                                    given,
                                    selected,
                                }],
                                row_clues: vec![],
                                column_clues: vec![],
                            },
                        }],
                    );
                    let mut surface = Surface::new(1072, 1448);
                    render_with(
                        &screen,
                        &CLARA_BW_METRICS,
                        &Chrome::default(),
                        &mut surface,
                        None,
                    );
                    assert!(
                        images.insert(surface.pixels.clone()),
                        "{mark:?} given={given} selected={selected}"
                    );
                }
            }
        }
        let mut surface = Surface::new(100, 100);
        let before = surface.pixels.clone();
        let rect = Rect {
            x: 30,
            y: 30,
            width: 20,
            height: 20,
        };
        draw_mark(
            &mut surface,
            rect,
            BoardMark::Value(u16::MAX),
            true,
            &CLARA_BW_METRICS,
            Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
        );
        for y in 0..100 {
            for x in 0..100 {
                if !rect.contains(x, y) {
                    assert_eq!(
                        surface.pixels[y as usize * 100 + x as usize],
                        before[y as usize * 100 + x as usize]
                    );
                }
            }
        }
    }
}
