//! A visible window never renumbers the puzzle or changes its stored cells.
use super::{Board, ScreenBuilder};
use crate::{BandAlign, ControlState, SlotWidth};
use kobo_ui::{BoardCell, BoardClue, BoardSurface, DisplayMetrics, FontSize, Node, Space};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Zoom {
    #[default]
    Standard,
    Large,
    ExtraLarge,
}
impl Zoom {
    const fn tenth_mm(self) -> u16 {
        match self {
            Self::Standard => 90,
            Self::Large => 120,
            Self::ExtraLarge => 160,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewError {
    NoRoom,
    Clues,
    DifferentBoard,
}
impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoRoom => "There is not enough room to show a square at this size.",
            Self::Clues => "The clues do not match this board.",
            Self::DifferentBoard => "This view belongs to a different board size.",
        })
    }
}
impl std::error::Error for ViewError {}

/// Complete clues, kept outside the wire viewport. Empty axes omit that gutter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardClues {
    pub rows: Vec<Vec<u8>>,
    pub columns: Vec<Vec<u8>>,
}
impl BoardClues {
    fn valid(&self, board: &Board) -> bool {
        (self.rows.is_empty() || self.rows.len() == board.rows())
            && (self.columns.is_empty() || self.columns.len() == board.columns())
            && self
                .rows
                .iter()
                .chain(&self.columns)
                .all(|line| line.len() <= 32 && line.iter().all(|value| (1..=64).contains(value)))
    }
}

#[derive(Clone, Debug)]
pub struct BoardViewport {
    board_columns: usize,
    board_rows: usize,
    column: usize,
    row: usize,
    columns: usize,
    rows: usize,
    width: i32,
    height: i32,
    metrics: DisplayMetrics,
    row_clues: bool,
    column_clues: bool,
    cell_tenth_mm: u16,
    zoom: Zoom,
}
impl BoardViewport {
    fn adjacent_zoom(&self, larger: bool) -> Option<Zoom> {
        match (self.zoom, larger) {
            (Zoom::Standard, true) | (Zoom::ExtraLarge, false) => Some(Zoom::Large),
            (Zoom::Large, true) => Some(Zoom::ExtraLarge),
            (Zoom::Large, false) => Some(Zoom::Standard),
            _ => None,
        }
    }
    #[must_use]
    pub fn can_resize(&self, larger: bool) -> bool {
        let Some(zoom) = self.adjacent_zoom(larger) else {
            return false;
        };
        let mut next = self.clone();
        next.zoom = zoom;
        next.fit().is_ok() && next.cell_tenth_mm != self.cell_tenth_mm
    }
    /// Handle the six shared navigation controls. Unknown/disabled actions
    /// return false. The board and its saved state are never changed.
    ///
    /// # Errors
    /// Refuses a requested size that cannot fit one square.
    pub fn navigate(&mut self, name: &str, selected: Option<usize>) -> Result<bool, ViewError> {
        let direction = match name {
            "board.left" => Some(Direction::Left),
            "board.right" => Some(Direction::Right),
            "board.up" => Some(Direction::Up),
            "board.down" => Some(Direction::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            return Ok(self.pan(direction));
        }
        let larger = match name {
            "board.larger" => true,
            "board.smaller" => false,
            _ => return Ok(false),
        };
        if !self.can_resize(larger) {
            return Ok(false);
        }
        let Some(zoom) = self.adjacent_zoom(larger) else {
            return Ok(false);
        };
        self.reflow(self.metrics, self.width, self.height, zoom, selected)?;
        Ok(true)
    }
    /// `width` and `height` are the space reserved for the board, after chrome,
    /// status, clue details and controls. Refuses a view with no usable square.
    ///
    /// # Errors
    /// Refuses invalid clues or an area too small for one accessible square.
    pub fn new(
        board: &Board,
        clues: &BoardClues,
        metrics: DisplayMetrics,
        width: i32,
        height: i32,
    ) -> Result<Self, ViewError> {
        if !clues.valid(board) {
            return Err(ViewError::Clues);
        }
        let mut view = Self {
            board_columns: board.columns(),
            board_rows: board.rows(),
            column: 0,
            row: 0,
            columns: 1,
            rows: 1,
            width,
            height,
            metrics,
            row_clues: !clues.rows.is_empty(),
            column_clues: !clues.columns.is_empty(),
            cell_tenth_mm: 90,
            zoom: Zoom::Standard,
        };
        view.fit()?;
        if let Some(cell) = board.selected() {
            view.reveal(cell);
        }
        Ok(view)
    }
    fn fit(&mut self) -> Result<(), ViewError> {
        kobo_ui::with_text_scale(self.metrics.text_scale, || {
            let (left, top) =
                BoardSurface::gutters(&self.metrics, self.row_clues, self.column_clues);
            let minimum = self
                .metrics
                .touch_target_default()
                .max(FontSize::Caption.line_height() * 2 + self.metrics.space(Space::Tight) * 2);
            let mut tenths = self.zoom.tenth_mm();
            while tenths < 300 && self.metrics.tenth_mm(i32::from(tenths)) < minimum {
                tenths += 1;
            }
            let cell = self.metrics.tenth_mm(i32::from(tenths));
            let gap = self.metrics.rule_thickness().max(1);
            let columns = usize::try_from(
                (self.width.saturating_sub(left).saturating_add(gap) / (cell + gap)).clamp(0, 12),
            )
            .unwrap_or(0);
            let rows = usize::try_from(
                (self.height.saturating_sub(top).saturating_add(gap) / (cell + gap)).clamp(0, 12),
            )
            .unwrap_or(0);
            if columns == 0 || rows == 0 || cell < minimum {
                return Err(ViewError::NoRoom);
            }
            self.columns = columns.min(self.board_columns);
            self.rows = rows.min(self.board_rows).min(81 / self.columns);
            self.cell_tenth_mm = tenths;
            self.column = self.column.min(self.board_columns - self.columns);
            self.row = self.row.min(self.board_rows - self.rows);
            Ok(())
        })
    }
    #[must_use]
    pub const fn zoom(&self) -> Zoom {
        self.zoom
    }
    #[must_use]
    pub fn visible_rows(&self) -> std::ops::Range<usize> {
        self.row..self.row + self.rows
    }
    #[must_use]
    pub fn visible_columns(&self) -> std::ops::Range<usize> {
        self.column..self.column + self.columns
    }
    pub fn cells(&self) -> impl Iterator<Item = usize> + '_ {
        self.visible_rows().flat_map(move |row| {
            self.visible_columns()
                .map(move |column| row * self.board_columns + column)
        })
    }
    #[must_use]
    pub fn contains(&self, cell: usize) -> bool {
        cell < self.board_rows * self.board_columns
            && self.visible_rows().contains(&(cell / self.board_columns))
            && self
                .visible_columns()
                .contains(&(cell % self.board_columns))
    }
    /// Returns false for an invalid cell; never changes the selection or marks.
    pub fn reveal(&mut self, cell: usize) -> bool {
        if cell >= self.board_rows * self.board_columns {
            return false;
        }
        let row = cell / self.board_columns;
        let column = cell % self.board_columns;
        self.row = self.row.min(row).max(row.saturating_sub(self.rows - 1));
        self.column = self
            .column
            .min(column)
            .max(column.saturating_sub(self.columns - 1));
        true
    }
    #[must_use]
    pub fn can_pan(&self, direction: Direction) -> bool {
        match direction {
            Direction::Left => self.column > 0,
            Direction::Right => self.column + self.columns < self.board_columns,
            Direction::Up => self.row > 0,
            Direction::Down => self.row + self.rows < self.board_rows,
        }
    }
    /// Pan one window with one row/column overlap when possible.
    pub fn pan(&mut self, direction: Direction) -> bool {
        if !self.can_pan(direction) {
            return false;
        }
        match direction {
            Direction::Left => {
                self.column = self
                    .column
                    .saturating_sub(self.columns.saturating_sub(1).max(1));
            }
            Direction::Right => {
                self.column = (self.column + self.columns.saturating_sub(1).max(1))
                    .min(self.board_columns - self.columns);
            }
            Direction::Up => self.row = self.row.saturating_sub(self.rows.saturating_sub(1).max(1)),
            Direction::Down => {
                self.row = (self.row + self.rows.saturating_sub(1).max(1))
                    .min(self.board_rows - self.rows);
            }
        }
        true
    }
    /// Refit atomically after rotation, resizing or a square-size change.
    ///
    /// # Errors
    /// On refusal the previous view remains unchanged.
    pub fn reflow(
        &mut self,
        metrics: DisplayMetrics,
        width: i32,
        height: i32,
        zoom: Zoom,
        selected: Option<usize>,
    ) -> Result<(), ViewError> {
        let mut next = self.clone();
        next.metrics = metrics;
        next.width = width;
        next.height = height;
        next.zoom = zoom;
        next.fit()?;
        let anchor = selected.unwrap_or(
            (self.row + self.rows / 2) * self.board_columns + self.column + self.columns / 2,
        );
        next.reveal(anchor);
        *self = next;
        Ok(())
    }
    #[must_use]
    pub fn position(&self) -> String {
        format!(
            "Rows {}–{} of {} · columns {}–{} of {}",
            self.row + 1,
            self.row + self.rows,
            self.board_rows,
            self.column + 1,
            self.column + self.columns,
            self.board_columns
        )
    }
    /// Maps only a currently visible cell name; stale/offscreen actions refuse.
    #[must_use]
    pub fn cell_action(&self, name: &str) -> Option<usize> {
        let cell = name.strip_prefix("board.cell.")?.parse().ok()?;
        self.contains(cell).then_some(cell)
    }
    /// Complete clue text for a currently visible gutter action. The caller
    /// shows it in its ordinary details screen or modal, with Back/Close.
    #[must_use]
    pub fn inspect_clue(&self, name: &str, clues: &BoardClues) -> Option<(String, String)> {
        let (axis, index, lines) = if let Some(index) = name.strip_prefix("board.row.") {
            let index = index.parse::<usize>().ok()?;
            if !self.row_clues
                || clues.rows.len() != self.board_rows
                || !self.visible_rows().contains(&index)
            {
                return None;
            }
            ("Row", index, &clues.rows)
        } else {
            let index = name.strip_prefix("board.column.")?.parse::<usize>().ok()?;
            if !self.column_clues
                || clues.columns.len() != self.board_columns
                || !self.visible_columns().contains(&index)
            {
                return None;
            }
            ("Column", index, &clues.columns)
        };
        let values = lines.get(index)?;
        if values.len() > 32 || values.iter().any(|value| !(1..=64).contains(value)) {
            return None;
        }
        let text = if values.is_empty() {
            "No filled squares in this line.".into()
        } else {
            values
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(" · ")
        };
        Some((format!("{axis} {}", index + 1), text))
    }
}

impl ScreenBuilder {
    /// Add a viewport using stable absolute cell/clue action names. A clue
    /// action opens the complete line supplied in `clues`; it does not edit.
    ///
    /// # Errors
    /// Refuses mismatched board dimensions or clue axes.
    pub fn board_viewport(
        mut self,
        board: &Board,
        clues: &BoardClues,
        view: &BoardViewport,
    ) -> Result<Self, ViewError> {
        if board.columns() != view.board_columns || board.rows() != view.board_rows {
            return Err(ViewError::DifferentBoard);
        }
        if !clues.valid(board)
            || clues.rows.is_empty() == view.row_clues
            || clues.columns.is_empty() == view.column_clues
        {
            return Err(ViewError::Clues);
        }
        let cells = view
            .cells()
            .map(|cell| BoardCell {
                action: self.register(&format!("board.cell.{cell}")),
                mark: board.marks()[cell],
                given: board.is_given(cell) == Some(true),
                selected: board.selected() == Some(cell),
            })
            .collect();
        let row_clues = if clues.rows.is_empty() {
            Vec::new()
        } else {
            view.visible_rows()
                .map(|row| BoardClue {
                    action: self.register(&format!("board.row.{row}")),
                    values: clues.rows[row].clone(),
                })
                .collect()
        };
        let column_clues = if clues.columns.is_empty() {
            Vec::new()
        } else {
            view.visible_columns()
                .map(|column| BoardClue {
                    action: self.register(&format!("board.column.{column}")),
                    values: clues.columns[column].clone(),
                })
                .collect()
        };
        let id = self.next_id();
        self.nodes.push(Node::Board {
            id,
            surface: BoardSurface {
                columns: u8::try_from(view.columns).map_err(|_| ViewError::DifferentBoard)?,
                row_start: u8::try_from(view.row).map_err(|_| ViewError::DifferentBoard)?,
                column_start: u8::try_from(view.column).map_err(|_| ViewError::DifferentBoard)?,
                cell_tenth_mm: view.cell_tenth_mm,
                cells,
                row_clues,
                column_clues,
            },
        });
        Ok(self)
    }
    /// Fixed positions for navigation and square size. Names are board.left,
    /// board.up, board.right, board.smaller, board.down and board.larger.
    #[must_use]
    pub fn board_viewport_controls(mut self, view: &BoardViewport) -> Self {
        for row in [
            [
                ("board.left", "Left", view.can_pan(Direction::Left)),
                ("board.up", "Up", view.can_pan(Direction::Up)),
                ("board.right", "Right", view.can_pan(Direction::Right)),
            ],
            [
                ("board.smaller", "Smaller", view.can_resize(false)),
                ("board.down", "Down", view.can_pan(Direction::Down)),
                ("board.larger", "Larger", view.can_resize(true)),
            ],
        ] {
            self = self.band(
                BandAlign::Middle,
                row.map(|(name, label, enabled)| {
                    (SlotWidth::Fill, move |slot: Self| {
                        slot.button_with_state(
                            name,
                            label,
                            if enabled {
                                ControlState::Enabled
                            } else {
                                ControlState::Disabled
                            },
                        )
                    })
                }),
            );
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Field, Mark};
    use kobo_ui::{Chrome, LayoutKind, Orientation, TextScale};
    fn board() -> Board {
        Board::new(
            64,
            64,
            std::iter::repeat_n(Field::editable(Mark::Empty), 4096),
        )
        .unwrap()
    }
    fn clues() -> BoardClues {
        BoardClues {
            rows: vec![vec![1, 2, 3, 4, 5]; 64],
            columns: vec![vec![6, 1, 2, 3]; 64],
        }
    }
    #[test]
    fn every_large_board_cell_is_reachable_without_renumbering_or_mutation() {
        let board = board();
        let mut view =
            BoardViewport::new(&board, &clues(), kobo_ui::CLARA_BW_METRICS, 950, 700).unwrap();
        let mut reached = std::collections::BTreeSet::new();
        loop {
            loop {
                reached.extend(view.cells());
                if !view.pan(Direction::Right) {
                    break;
                }
            }
            while view.pan(Direction::Left) {}
            if !view.pan(Direction::Down) {
                break;
            }
        }
        assert_eq!(reached.len(), 4096);
        assert!(board.marks().iter().all(|mark| *mark == Mark::Empty));
        assert!(view.reveal(0));
        assert_eq!(view.cell_action("board.cell.0"), Some(0));
        assert_eq!(view.cell_action("board.cell.4095"), None);
        assert_eq!(view.cell_action("board.cell.4096"), None);
        assert_eq!(
            view.inspect_clue("board.row.0", &clues()),
            Some(("Row 1".into(), "1 · 2 · 3 · 4 · 5".into()))
        );
        assert!(view.inspect_clue("board.row.63", &clues()).is_none());
    }
    #[test]
    fn rotation_and_zoom_reveal_the_selection_and_refused_reflow_is_atomic() {
        let mut board = board();
        board.select(4095).unwrap();
        let metrics = kobo_ui::CLARA_BW_METRICS;
        let mut view = BoardViewport::new(&board, &clues(), metrics, 950, 700).unwrap();
        assert!(view.contains(4095));
        view.navigate("board.larger", board.selected()).unwrap();
        assert!(view.contains(4095));
        view.reflow(
            metrics.oriented(Orientation::Landscape),
            1300,
            450,
            Zoom::ExtraLarge,
            board.selected(),
        )
        .unwrap();
        assert!(view.contains(4095));
        let before = (
            view.position(),
            view.zoom(),
            view.cells().collect::<Vec<_>>(),
        );
        assert_eq!(
            view.reflow(metrics, 1, 1, Zoom::Standard, board.selected()),
            Err(ViewError::NoRoom)
        );
        assert_eq!(
            before,
            (view.position(), view.zoom(), view.cells().collect())
        );
        assert!(!view.reveal(4096));
    }
    #[cfg(feature = "text")]
    #[test]
    fn viewport_clues_and_controls_fit_every_profile_size_and_orientation() {
        let mut board = board();
        board
            .apply([
                (0, Mark::Filled),
                (1, Mark::Crossed),
                (2, Mark::Dot),
                (3, Mark::Notes(5)),
                (4, Mark::Value(7)),
            ])
            .unwrap();
        board.select(0).unwrap();
        let clues = clues();
        for profile in kobo_profile::SUPPORTED_PROFILES {
            for scale in [TextScale::Default, TextScale::Large, TextScale::ExtraLarge] {
                for orientation in [Orientation::Portrait, Orientation::Landscape] {
                    let metrics = DisplayMetrics {
                        width: i32::try_from(profile.width).unwrap(),
                        height: i32::try_from(profile.height).unwrap(),
                        pixels_per_inch: i32::from(profile.pixels_per_inch),
                        text_scale: scale,
                    }
                    .oriented(orientation);
                    kobo_text::install(metrics).unwrap();
                    let view = BoardViewport::new(
                        &board,
                        &clues,
                        metrics,
                        metrics.content_width(),
                        metrics.height * 2 / 5,
                    )
                    .unwrap();
                    let screen = ScreenBuilder::new("board-check")
                        .top_bar("Puzzle")
                        .board_viewport(&board, &clues, &view)
                        .unwrap()
                        .board_viewport_controls(&view)
                        .build_checked_with(&metrics, &Chrome::with_back(true))
                        .unwrap_or_else(|error| {
                            panic!("{} {scale:?} {orientation:?}: {error:?}", profile.id)
                        });
                    let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
                    for cell in view.cells() {
                        let action = crate::action_id(&format!("board.cell.{cell}"));
                        let rect = layout.rect_of_action(action).unwrap();
                        assert!(
                            rect.width >= metrics.touch_target_minimum()
                                && rect.height >= metrics.touch_target_minimum()
                        );
                        assert_eq!(
                            layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                            Some(action)
                        );
                    }
                    assert!(layout
                        .nodes
                        .iter()
                        .any(|node| node.kind == LayoutKind::BoardClue
                            && node.text_lines.iter().any(|line| line.contains('…'))));
                    assert!(layout.nodes.iter().any(|node| node
                        .text_lines
                        .iter()
                        .any(|line| line == "Row 1 clue: 1 2 3 4 5")));
                }
            }
        }
    }
}
