//! Attached clues and a measured window into the complete puzzle.
use super::{action_id, ActionId, Context, Game, Mark, Route, Screen, ScreenBuilder, Status};
use kobo_sdk::board::{Board, BoardClues, BoardViewport, Direction, Field, Mark as Ink};
use kobo_sdk::{BandAlign, Chrome, ControlState, SlotWidth, Space};

const HELP: &str = "Use the clues to fill the picture\n\nEach number is a run of filled squares in its row or column. Separate runs with at least one empty square. A zero means the entire line is empty.\n\nTap a square to cycle blank, filled and crossed out. The outlined square is your last choice. A cross means you have decided the square is empty.\n\nClues sit beside their matching lines. Tap a clue to read its complete sequence. An ellipsis means there are more numbers.\n\nLeft, Right, Up and Down move the window with an overlap. The − and + buttons change square size. Your marks stay in their original rows and columns.\n\nMore contains checking and run entry. Free accepts your choices without warnings. Guided names a row or column whose clues no longer fit.\n\nRun entry fills a horizontal or vertical run between two taps. Undo reverses the whole run, a single mark or a confirmed restart.\n\nMark every square, including empty squares with a cross, to finish. The completed picture opens when all marks match.";

fn state(enabled: bool) -> ControlState {
    if enabled {
        ControlState::Enabled
    } else {
        ControlState::Disabled
    }
}
impl Game {
    pub(super) fn ink_board(&self) -> Option<(Board, BoardClues)> {
        let puzzle = self.puzzle()?;
        let mut board = Board::new(
            puzzle.side,
            puzzle.side,
            self.marks.iter().map(|mark| {
                Field::editable(match mark {
                    Mark::Blank => Ink::Empty,
                    Mark::Fill => Ink::Filled,
                    Mark::Cross => Ink::Crossed,
                })
            }),
        )
        .ok()?;
        if let Some(cell) = self.focus {
            let _ = board.select(cell);
        }
        Some((
            board,
            BoardClues {
                rows: puzzle.row_clues(),
                columns: puzzle.column_clues(),
            },
        ))
    }
    fn play_header(&self, wide: bool) -> ScreenBuilder {
        let side = self.puzzle().map_or(1, |puzzle| puzzle.side);
        let position = self.focus.map_or_else(
            || "Choose a square".into(),
            |cell| format!("Row {} · Column {}", cell / side + 1, cell % side + 1),
        );
        let position = if let Some(view) = self
            .viewport
            .as_ref()
            .filter(|view| self.focus.is_some_and(|cell| !view.contains(cell)))
        {
            let rows = view.visible_rows();
            let columns = view.visible_columns();
            format!(
                "Rows {}–{} · Cols {}–{}",
                rows.start + 1,
                rows.end,
                columns.start + 1,
                columns.end
            )
        } else {
            position
        };
        let status = match self.draft.status() {
            Status::Failed(_) => "Not saved · More to retry".into(),
            Status::Saving | Status::Unsaved => "Saving…".into(),
            Status::Saved if self.run_start.is_some() => "Tap the other end of the run".into(),
            Status::Saved if self.notice.is_some() => self.notice.clone().unwrap_or_default(),
            Status::Saved => format!(
                "{position} · {}",
                if self.guided { "Guided" } else { "Free" }
            ),
        };
        let builder = ScreenBuilder::new("nonograms-play").top_bar("Nonograms");
        if wide {
            builder
        } else {
            builder.secondary(status)
        }
    }
    fn play_tools(
        &self,
        builder: ScreenBuilder,
        view: &BoardViewport,
        wide: bool,
    ) -> ScreenBuilder {
        if wide {
            let mut builder = builder;
            for row in [
                [
                    ("board.left", "Left", view.can_pan(Direction::Left)),
                    ("board.up", "Up", view.can_pan(Direction::Up)),
                    ("board.right", "Right", view.can_pan(Direction::Right)),
                    ("board.down", "Down", view.can_pan(Direction::Down)),
                ],
                [
                    ("board.smaller", "−", view.can_resize(false)),
                    ("board.larger", "+", view.can_resize(true)),
                    ("undo", "Undo", !self.undo.is_empty()),
                    ("more", "More", true),
                ],
            ] {
                builder = builder.band(
                    BandAlign::Middle,
                    row.map(|(name, label, enabled)| {
                        (SlotWidth::Fill, move |slot: ScreenBuilder| {
                            slot.button_with_state(name, label, state(enabled))
                        })
                    }),
                );
            }
            return builder;
        }
        let mut builder = builder;
        for row in [
            [
                ("board.left", "Left", view.can_pan(Direction::Left)),
                ("board.up", "Up", view.can_pan(Direction::Up)),
                ("board.right", "Right", view.can_pan(Direction::Right)),
            ],
            [
                ("board.smaller", "−", view.can_resize(false)),
                ("board.down", "Down", view.can_pan(Direction::Down)),
                ("board.larger", "+", view.can_resize(true)),
            ],
        ] {
            builder = builder.band(
                BandAlign::Middle,
                row.map(|(name, label, enabled)| {
                    (SlotWidth::Fill, move |slot: ScreenBuilder| {
                        slot.button_with_state(name, label, state(enabled))
                    })
                }),
            );
        }
        builder.band(
            BandAlign::Middle,
            [
                (
                    SlotWidth::Fill,
                    Box::new(|slot: ScreenBuilder| {
                        slot.button_with_state("undo", "Undo", state(!self.undo.is_empty()))
                    }) as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder + '_>,
                ),
                (
                    SlotWidth::Fill,
                    Box::new(|slot: ScreenBuilder| slot.button("more", "More"))
                        as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder + '_>,
                ),
            ],
        )
    }
    pub(super) fn fitted_view(
        &self,
        context: &Context,
        board: &Board,
        clues: &BoardClues,
    ) -> Option<BoardViewport> {
        let metrics = context.metrics();
        let probe = BoardViewport::new(
            board,
            clues,
            metrics,
            metrics.content_width(),
            metrics.height,
        )
        .ok()?;
        let skeleton = self
            .play_tools(
                self.play_header(metrics.width > metrics.height),
                &probe,
                metrics.width > metrics.height,
            )
            .build();
        let layout = skeleton.layout_with(&metrics, &Chrome::measuring(true));
        let height = layout.content.height - layout.flow_height - metrics.space(Space::Medium) * 2;
        let mut view = self.viewport.clone().unwrap_or(probe);
        // Reflow only for the allocated space. Retain a panned window even when
        // its previously selected square is now outside the window.
        view.reflow(
            metrics,
            metrics.content_width(),
            height,
            view.zoom(),
            if self.viewport.is_none() {
                self.focus
            } else {
                None
            },
        )
        .ok()?;
        Some(view)
    }
    pub(super) fn play(&self, context: &Context) -> Screen {
        let Some((board, clues)) = self.ink_board() else {
            return self.browser(context);
        };
        let Some(view) = self.fitted_view(context, &board, &clues) else {
            return ScreenBuilder::new("nonograms-no-room")
                .top_bar("Nonograms")
                .text("The board cannot fit at this display size.")
                .button("back-browser", "Back to puzzles")
                .build();
        };
        self.play_tools(
            self.play_header(context.metrics().width > context.metrics().height)
                .board_viewport(&board, &clues, &view)
                .expect("matching board"),
            &view,
            context.metrics().width > context.metrics().height,
        )
        .build()
    }
    pub(super) fn menu(&self) -> Screen {
        if matches!(self.draft.status(), Status::Failed(_))
            || matches!(self.solved_draft.status(), Status::Failed(_))
        {
            return ScreenBuilder::new("nonograms-save-recovery")
                .top_bar("Not saved")
                .text("Your latest marks are still here. Retry before closing this puzzle.")
                .button("retry-save", "Retry save")
                .button("resume", "Resume")
                .build();
        }

        let mut screen = ScreenBuilder::new("nonograms-menu")
            .top_bar("Options")
            .secondary(self.puzzle().map_or_else(
                || "Nonograms".into(),
                |p| format!("{} · {}", p.title, p.difficulty()),
            ));
        if let Some(notice) = &self.notice {
            screen = screen.text(notice);
        }
        screen
            .grid(
                2,
                false,
                [
                    ("policy", if self.guided { "Guided" } else { "Free" }),
                    (
                        "run-entry",
                        if self.run_entry { "Run on" } else { "Run off" },
                    ),
                    ("reset", "Restart"),
                    ("resume", "Resume"),
                ],
            )
            .button_with_state("undo", "Undo", state(!self.undo.is_empty()))
            .button("back-browser", "Puzzles")
            .build()
    }
    pub(super) fn clue_screen(&self, context: &Context) -> Screen {
        let (title, clue) = self.clue.as_ref().expect("opened clue");
        let _ = context;
        // A supported 25-square line has at most 13 runs, but measure its
        // complete text rather than truncating it to the gutter preview.
        ScreenBuilder::new("nonograms-clue")
            .top_bar(title)
            .text(clue)
            .bottom_action("resume", "Back to puzzle")
            .build()
    }
    pub(super) fn help_pages(&self, context: &Context) -> Vec<Vec<String>> {
        context.paginate(if self.route == Route::PhotoHelp {
            "Send a photo

On your computer, run:

kobo nonograms push IMAGE --size N --device READER

Replace IMAGE with your image file, N with 5, 7 or 9, and READER with your reader address. Choose the same size in Photo puzzle, then Open.

Only puzzles solvable by row and column deductions are accepted. Try another photo or size if the clues need guessing.

Easy needs one solving pass, Medium two or three, and Hard more. This is a repeatable solver rating, not a prediction of your solving time."
        } else { HELP }, true)
    }
    pub(super) fn help(&self, context: &Context) -> Screen {
        let pages = self.help_pages(context);
        let page = self.help_page.min(pages.len().saturating_sub(1));
        let mut builder = ScreenBuilder::new("nonograms-help").top_bar("How to play");
        for paragraph in &pages[page] {
            builder = builder.text(paragraph);
        }
        builder
            .page_turns("help-previous", "help-next")
            .page_position(
                u16::try_from(page + 1).expect("bounded help"),
                u16::try_from(pages.len().max(1)).expect("bounded help"),
            )
            .action_bar([("help-previous", "Previous"), ("help-next", "Next")])
            .build()
    }
    pub(super) fn remember(&mut self) {
        if self.undo.len() == 64 {
            self.undo.pop_front();
        }
        self.undo.push_back(self.marks.clone());
    }
    pub(super) fn board_action(&mut self, context: &mut Context, action: ActionId) {
        let Some((board, clues)) = self.ink_board() else {
            return;
        };
        let Some(mut view) = self.fitted_view(context, &board, &clues) else {
            return;
        };
        let nav = [
            "board.left",
            "board.right",
            "board.up",
            "board.down",
            "board.smaller",
            "board.larger",
        ];
        if let Some(name) = nav.into_iter().find(|name| action == action_id(name)) {
            let _ = view.navigate(name, self.focus);
        } else if let Some(cell) = view
            .cells()
            .find(|cell| action == action_id(&format!("board.cell.{cell}")))
        {
            if self.run_entry {
                self.enter_run(context, cell);
            } else {
                self.toggle(context, cell);
            }
        } else {
            let names = view
                .visible_rows()
                .map(|row| format!("board.row.{row}"))
                .chain(
                    view.visible_columns()
                        .map(|column| format!("board.column.{column}")),
                );
            if let Some(name) = names.into_iter().find(|name| action == action_id(name)) {
                if let Some(index) = name
                    .strip_prefix("board.row.")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    self.focus = Some(index * board.columns() + view.visible_columns().start);
                } else if let Some(index) = name
                    .strip_prefix("board.column.")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    self.focus = Some(view.visible_rows().start * board.columns() + index);
                }
                self.clue = view.inspect_clue(&name, &clues);
                if self.clue.is_some() {
                    self.route = Route::Clue;
                }
            }
        }
        self.viewport = Some(view);
    }
}
