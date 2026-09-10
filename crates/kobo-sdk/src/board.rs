//! Bounded board edits and consistent undo controls.
//!
//! A move may change several cells (a run, candidate removal or reset). It is
//! accepted or refused as a whole, and one Undo reverses the whole move. Givens
//! cannot be edited. Apps retain game rules, clue data and durable storage;
//! changing this in-memory board never means that a save was acknowledged.

use crate::{BandAlign, ControlState, ScreenBuilder, SlotWidth};
use std::collections::{BTreeSet, VecDeque};

pub const MAX_SIDE: usize = 64;
pub const MAX_BOARD_CELLS: usize = MAX_SIDE * MAX_SIDE;
pub const MAX_UNDO_MOVES: usize = 64;
/// Shared by undo and redo. Large multi-cell moves shorten retained history.
pub const MAX_HISTORY_CHANGES: usize = 8192;
pub const UNDO: &str = "board.undo";
pub const REDO: &str = "board.redo";
pub const CLEAR: &str = "board.clear";

/// Game meaning belongs to the app. Values may name a number, letter or piece;
/// notes are a set of at most 32 candidates. No mark relies on color alone.
pub use kobo_ui::BoardMark as Mark;
mod viewport;
pub use viewport::{BoardClues, BoardViewport, Direction, ViewError, Zoom};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Field {
    pub mark: Mark,
    pub locked: bool,
}
impl Field {
    #[must_use]
    pub const fn editable(mark: Mark) -> Self {
        Self {
            mark,
            locked: false,
        }
    }
    #[must_use]
    pub const fn given(mark: Mark) -> Self {
        Self { mark, locked: true }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardError {
    Dimensions,
    CellCount,
    InvalidMark,
    OutsideBoard,
    Given,
    DuplicateCell,
    TooManyEdits,
}
impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dimensions => "Board dimensions must be between 1 and 64.",
            Self::CellCount => "The cells do not match the board dimensions.",
            Self::InvalidMark => {
                "Use an empty mark instead of a zero value or empty candidate set."
            }
            Self::OutsideBoard => "That square is outside this board.",
            Self::Given => "This square is part of the puzzle and cannot be changed.",
            Self::DuplicateCell => "A move cannot name the same square twice.",
            Self::TooManyEdits => "A move contains more edits than this board has squares.",
        })
    }
}
impl std::error::Error for BoardError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Change {
    cell: usize,
    before: Mark,
    after: Mark,
}

#[derive(Clone, Debug)]
pub struct Board {
    columns: usize,
    rows: usize,
    initial: Vec<Field>,
    marks: Vec<Mark>,
    selected: Option<usize>,
    undo: VecDeque<Vec<Change>>,
    redo: Vec<Vec<Change>>,
}
impl Board {
    /// Create a board. Input is bounded before it is collected; invalid input
    /// cannot silently truncate a puzzle.
    ///
    /// # Errors
    /// Refuses invalid dimensions, cell counts and zero-valued marks.
    pub fn new(
        columns: usize,
        rows: usize,
        fields: impl IntoIterator<Item = Field>,
    ) -> Result<Self, BoardError> {
        if !(1..=MAX_SIDE).contains(&columns) || !(1..=MAX_SIDE).contains(&rows) {
            return Err(BoardError::Dimensions);
        }
        let count = columns * rows;
        let initial: Vec<_> = fields.into_iter().take(count + 1).collect();
        if initial.len() != count {
            return Err(BoardError::CellCount);
        }
        if initial.iter().any(|field| !field.mark.is_valid()) {
            return Err(BoardError::InvalidMark);
        }
        Ok(Self {
            columns,
            rows,
            marks: initial.iter().map(|field| field.mark).collect(),
            initial,
            selected: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
        })
    }

    /// Restore app-validated saved marks without creating imaginary undo
    /// history. Refuses changed givens; the caller can retain the original save
    /// for recovery. Puzzle identity/version validation belongs to the app.
    ///
    /// # Errors
    /// Refuses mismatched counts, invalid marks and changed givens without mutation.
    pub fn restore(&mut self, marks: &[Mark]) -> Result<(), BoardError> {
        if marks.len() != self.initial.len() {
            return Err(BoardError::CellCount);
        }
        for (field, mark) in self.initial.iter().zip(marks) {
            if !mark.is_valid() {
                return Err(BoardError::InvalidMark);
            }
            if field.locked && field.mark != *mark {
                return Err(BoardError::Given);
            }
        }
        self.marks.copy_from_slice(marks);
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    #[must_use]
    pub const fn columns(&self) -> usize {
        self.columns
    }
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }
    #[must_use]
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }
    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        self.selected
    }
    #[must_use]
    pub fn is_given(&self, cell: usize) -> Option<bool> {
        self.initial.get(cell).map(|field| field.locked)
    }
    /// Select an existing square, including a given for clue inspection.
    ///
    /// # Errors
    /// Refuses an index outside this board.
    pub fn select(&mut self, cell: usize) -> Result<(), BoardError> {
        if cell >= self.marks.len() {
            return Err(BoardError::OutsideBoard);
        }
        self.selected = Some(cell);
        Ok(())
    }
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    #[must_use]
    pub fn can_clear(&self) -> bool {
        self.selected
            .is_some_and(|cell| !self.initial[cell].locked && self.marks[cell] != Mark::Empty)
    }

    /// Apply one atomic move. Refused and unchanged moves preserve redo. A new
    /// move after Undo discards the abandoned future, never the current board.
    ///
    /// # Errors
    /// Refuses excessive edits, duplicate/out-of-range squares, invalid marks
    /// and edits to givens before changing any square.
    pub fn apply(
        &mut self,
        edits: impl IntoIterator<Item = (usize, Mark)>,
    ) -> Result<bool, BoardError> {
        let mut seen = BTreeSet::new();
        let mut changes = Vec::new();
        for (position, (cell, after)) in edits.into_iter().enumerate() {
            if position >= self.marks.len() {
                return Err(BoardError::TooManyEdits);
            }
            let field = self.initial.get(cell).ok_or(BoardError::OutsideBoard)?;
            if !seen.insert(cell) {
                return Err(BoardError::DuplicateCell);
            }
            if !after.is_valid() {
                return Err(BoardError::InvalidMark);
            }
            if field.locked && after != field.mark {
                return Err(BoardError::Given);
            }
            let before = self.marks[cell];
            if before != after {
                changes.push(Change {
                    cell,
                    before,
                    after,
                });
            }
        }
        if changes.is_empty() {
            return Ok(false);
        }
        for change in &changes {
            self.marks[change.cell] = change.after;
        }
        self.redo.clear();
        self.undo.push_back(changes);
        while self.undo.len() > MAX_UNDO_MOVES || self.history_changes() > MAX_HISTORY_CHANGES {
            self.undo.pop_front();
        }
        Ok(true)
    }

    /// Focus the first affected square so an app can reveal it after an Undo.
    pub fn undo(&mut self) -> bool {
        let Some(changes) = self.undo.pop_back() else {
            return false;
        };
        for change in &changes {
            self.marks[change.cell] = change.before;
        }
        self.selected = changes.first().map(|change| change.cell);
        self.redo.push(changes);
        true
    }
    pub fn redo(&mut self) -> bool {
        let Some(changes) = self.redo.pop() else {
            return false;
        };
        for change in &changes {
            self.marks[change.cell] = change.after;
        }
        self.selected = changes.first().map(|change| change.cell);
        self.undo.push_back(changes);
        true
    }
    /// Clear the selected editable square as one undoable move.
    ///
    /// # Errors
    /// Refuses clearing a nonempty given. No selection is an unchanged result.
    pub fn clear_selected(&mut self) -> Result<bool, BoardError> {
        let Some(cell) = self.selected else {
            return Ok(false);
        };
        self.apply([(cell, Mark::Empty)])
    }

    /// Reset editable squares as one undoable move. Apps must ask before
    /// resetting a played puzzle; this method itself does not show a dialog.
    ///
    /// # Errors
    /// Propagates move validation errors; validated initial fields preserve
    /// the same dimensions and givens.
    pub fn reset(&mut self) -> Result<bool, BoardError> {
        let edits: Vec<_> = self
            .initial
            .iter()
            .enumerate()
            .filter(|(_, field)| !field.locked)
            .map(|(cell, field)| (cell, field.mark))
            .collect();
        self.apply(edits)
    }

    fn history_changes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Vec::len).sum()
    }
}

impl ScreenBuilder {
    /// Reserve the same three named controls throughout the puzzle. Disabled
    /// controls retain their places; dispatch calls `undo`, `redo` or
    /// `clear_selected`, then saves the changed board using acknowledged state.
    #[must_use]
    pub fn board_history_controls(self, board: &Board) -> Self {
        self.band(
            BandAlign::Middle,
            [
                (UNDO, "Undo", board.can_undo()),
                (REDO, "Redo", board.can_redo()),
                (CLEAR, "Clear", board.can_clear()),
            ]
            .map(|(name, label, enabled)| {
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
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn puzzle() -> Board {
        Board::new(
            3,
            2,
            [Field::given(Mark::Value(1))]
                .into_iter()
                .chain(std::iter::repeat_n(Field::editable(Mark::Empty), 5)),
        )
        .unwrap()
    }

    #[test]
    fn run_entry_is_one_move_and_undo_reveals_its_first_square() {
        let mut board = puzzle();
        board
            .apply([
                (1, Mark::Filled),
                (2, Mark::Crossed),
                (3, Mark::Notes(0b101)),
            ])
            .unwrap();
        let played = board.marks().to_vec();
        board.select(5).unwrap();
        assert!(board.undo());
        assert_eq!(board.selected(), Some(1));
        assert_eq!(board.marks(), puzzle().marks());
        assert!(!board.undo());
        assert!(board.redo());
        assert_eq!(board.marks(), played);
        assert!(!board.redo());
    }

    #[test]
    fn invalid_partial_edits_never_change_board_or_abandon_redo() {
        let mut board = puzzle();
        board.apply([(1, Mark::Filled)]).unwrap();
        board.undo();
        let original = board.marks().to_vec();
        for edits in [
            vec![(1, Mark::Dot), (0, Mark::Empty)],
            vec![(1, Mark::Dot), (9, Mark::Empty)],
            vec![(1, Mark::Dot), (1, Mark::Filled)],
            vec![(1, Mark::Dot), (2, Mark::Notes(0))],
        ] {
            assert!(board.apply(edits).is_err());
            assert_eq!(board.marks(), original);
            assert!(board.can_redo());
        }
        assert!(!board.apply([(1, Mark::Empty)]).unwrap());
        assert!(board.can_redo());
        board.apply([(2, Mark::Crossed)]).unwrap();
        assert!(!board.can_redo());
    }

    #[test]
    fn reset_and_clear_preserve_givens_and_can_be_undone() {
        let mut board = puzzle();
        board.select(0).unwrap();
        assert!(!board.can_clear());
        assert_eq!(board.clear_selected(), Err(BoardError::Given));
        board.select(2).unwrap();
        assert!(!board.can_clear());
        board.apply([(2, Mark::Value(5))]).unwrap();
        assert!(board.can_clear());
        board.clear_selected().unwrap();
        board.undo();
        assert_eq!(board.marks()[2], Mark::Value(5));
        board.reset().unwrap();
        assert_eq!(board.marks(), puzzle().marks());
        board.undo();
        assert_eq!(board.marks()[2], Mark::Value(5));
        assert_eq!(board.marks()[0], Mark::Value(1));
    }

    #[test]
    fn full_board_moves_bound_history_across_undo_and_redo() {
        let mut board = Board::new(
            64,
            64,
            std::iter::repeat_n(Field::editable(Mark::Empty), MAX_BOARD_CELLS),
        )
        .unwrap();
        for value in 1..=5 {
            board
                .apply((0..MAX_BOARD_CELLS).map(|cell| (cell, Mark::Value(value))))
                .unwrap();
            assert!(board.history_changes() <= MAX_HISTORY_CHANGES);
        }
        assert!(board.undo());
        assert!(board.undo());
        assert!(!board.undo());
        assert!(board.marks().iter().all(|mark| *mark == Mark::Value(3)));
        assert_eq!(board.history_changes(), MAX_HISTORY_CHANGES);
        assert!(board.redo());
        assert!(board.redo());
        assert!(!board.redo());
        assert!(board.marks().iter().all(|mark| *mark == Mark::Value(5)));
    }

    #[test]
    fn small_moves_are_also_count_bounded() {
        let mut board = puzzle();
        for value in 1..=100 {
            board.apply([(1, Mark::Value(value))]).unwrap();
        }
        for _ in 0..MAX_UNDO_MOVES {
            assert!(board.undo());
        }
        assert!(!board.undo());
        assert_eq!(board.marks()[1], Mark::Value(36));
    }

    #[test]
    fn restore_is_atomic_and_does_not_invent_history() {
        let mut board = puzzle();
        board.apply([(1, Mark::Value(2))]).unwrap();
        let good = board.marks().to_vec();
        let mut bad = good.clone();
        bad[0] = Mark::Empty;
        assert_eq!(board.restore(&bad), Err(BoardError::Given));
        assert_eq!(board.marks(), good);
        assert!(board.can_undo());
        board.restore(&good).unwrap();
        assert!(!board.can_undo() && !board.can_redo());
        assert_eq!(board.marks(), good);
    }

    #[test]
    fn invalid_dimensions_and_unbounded_input_are_refused() {
        assert!(matches!(
            Board::new(usize::MAX, 2, []),
            Err(BoardError::Dimensions)
        ));
        assert!(matches!(Board::new(0, 1, []), Err(BoardError::Dimensions)));
        assert!(matches!(
            Board::new(3, 2, std::iter::repeat(Field::editable(Mark::Empty))),
            Err(BoardError::CellCount)
        ));
        let mut board = puzzle();
        assert!(board.apply(std::iter::repeat((1, Mark::Filled))).is_err());
        assert_eq!(board.marks(), puzzle().marks());
    }
    #[cfg(feature = "text")]
    #[test]
    fn history_controls_stay_in_place_and_only_enable_available_actions() {
        use crate::{action_id, Chrome, DisplayMetrics, Orientation};
        use kobo_ui::{LayoutKind, TextScale};
        let mut played = puzzle();
        played.select(1).unwrap();
        played.apply([(1, Mark::Filled)]).unwrap();
        let mut undone = played.clone();
        undone.undo();
        let states = [puzzle(), played, undone];
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
                    let mut previous = None;
                    for board in &states {
                        let chrome = Chrome::with_back(true);
                        let screen = ScreenBuilder::new("puzzle")
                            .top_bar("Puzzle")
                            .text("Select a square to change it.")
                            .board_history_controls(board)
                            .build_checked_with(&metrics, &chrome)
                            .unwrap();
                        let layout = screen.layout_with(&metrics, &chrome);
                        let mut rects = Vec::new();
                        for (name, enabled) in [
                            (UNDO, board.can_undo()),
                            (REDO, board.can_redo()),
                            (CLEAR, board.can_clear()),
                        ] {
                            let node = layout.nodes.iter().find(|node| matches!(node.kind, LayoutKind::Button(id, _, _) if id == action_id(name))).unwrap();
                            assert_eq!(node.kind.acts_on().is_some(), enabled);
                            rects.push(node.rect);
                        }
                        if let Some(ref previous) = previous {
                            assert_eq!(previous, &rects);
                        }
                        previous = Some(rects);
                    }
                }
            }
        }
    }
}
