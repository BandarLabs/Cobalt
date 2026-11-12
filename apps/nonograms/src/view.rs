//! Attached clues and a measured window into the complete puzzle.
use super::{action_id, ActionId, Context, Game, Mark, Route, Screen, ScreenBuilder, Status};
use kobo_sdk::board::{Board, BoardClues, BoardViewport, Direction, Field, Mark as Ink};
use kobo_sdk::{BandAlign, Chrome, ControlState, SlotWidth, Space};

const HELP: &str = "Use the clues to fill the picture\n\nEach number is a run of filled squares in its row or column. Separate runs with at least one empty square. A zero means the entire line is empty.\n\nTap a square to cycle blank, filled and crossed out. The outlined square is your last choice. A cross means you have decided the square is empty.\n\nClues sit beside their matching lines. Tap a clue to read its complete sequence. An ellipsis means there are more numbers.\n\nOn a board too large to show whole, Left, Right, Up and Down move the window with an overlap. The − and + buttons change square size. Your marks stay in their original rows and columns.\n\nMore holds two switches. With Guided off, your choices are accepted without warnings. Guided names a row or column whose clues no longer fit.\n\nRun entry fills a horizontal or vertical run between two taps. Undo reverses the whole run, a single mark or a confirmed restart.\n\nMark every square, including empty squares with a cross, to finish. The completed picture opens when all marks match.";

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
    fn play_status(&self) -> String {
        let side = self.puzzle().map_or(1, |puzzle| puzzle.side);
        // The focused square is visible on the board; the header carries
        // coordinates only when a zoomed window leaves the focus outside the
        // view and the window's place is otherwise invisible.
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
        } else if self.focus.is_none() {
            "Choose a square".to_owned()
        } else {
            String::new()
        };
        match self.draft.status() {
            Status::Failed(_) => "Not saved · More to retry".into(),
            Status::Saving | Status::Unsaved => "Saving…".into(),
            Status::Saved if self.run_start.is_some() => "Tap the other end of the run".into(),
            Status::Saved if self.notice.is_some() => self.notice.clone().unwrap_or_default(),
            Status::Saved => {
                let total = side * side;
                let marked = self
                    .marks
                    .iter()
                    .filter(|mark| !matches!(mark, Mark::Blank))
                    .count();
                if position.is_empty() {
                    format!("{marked}/{total}")
                } else {
                    format!("{position} · {marked}/{total}")
                }
            }
        }
    }
    /// Every line the status can show on this board, the current one first.
    ///
    /// The board is fitted to the room the tallest of them leaves. Fitted to
    /// the current line alone, a status that wraps once the window is panned
    /// would take a row of squares away in the middle of a puzzle.
    fn status_lines(&self) -> Vec<String> {
        let side = self.puzzle().map_or(1, |puzzle| puzzle.side);
        let total = side * side;
        vec![
            self.play_status(),
            format!("Rows {side}–{side} · Cols {side}–{side} · {total}/{total}"),
            "Not saved · More to retry".into(),
            "Tap the other end of the run".into(),
        ]
    }
    /// The bar names the puzzle being played, as the shelf did when it was
    /// chosen.
    fn play_title(&self) -> String {
        self.puzzle()
            .map_or_else(|| "Nonograms".to_owned(), |puzzle| puzzle.title.clone())
    }
    fn play_header_with(title: String, status: String, wide: bool) -> ScreenBuilder {
        let builder = ScreenBuilder::new("nonograms-play").top_bar(title);
        if wide {
            builder
        } else {
            builder.secondary(status)
        }
    }
    fn play_header(&self, wide: bool) -> ScreenBuilder {
        Self::play_header_with(self.play_title(), self.play_status(), wide)
    }
    fn play_tools(
        &self,
        builder: ScreenBuilder,
        view: &BoardViewport,
        pans: bool,
    ) -> ScreenBuilder {
        // Upright only: held sideways the controls stand beside the board.
        if pans {
            return self.upright_pan_tools(builder, view);
        }
        // A board that is whole on the panel has nowhere to move, and four
        // greyed buttons that never wake are only in the way. They come
        // back as soon as + makes the squares too large to show at once.
        let rows = [
            [
                ("board.smaller", "−", view.can_resize(false)),
                ("board.larger", "+", view.can_resize(true)),
            ],
            [
                ("undo", "Undo", !self.undo.is_empty()),
                ("more", "More", true),
            ],
        ];
        let mut builder = builder;
        for row in rows {
            builder = builder.band(
                BandAlign::Middle,
                row.into_iter().map(|(name, label, enabled)| {
                    (SlotWidth::Fill, move |slot: ScreenBuilder| {
                        slot.button_with_state(name, label, state(enabled))
                    })
                }),
            );
        }
        builder
    }
    /// The pan buttons in a cross under an upright board, with the size
    /// buttons either side of Up.
    fn upright_pan_tools(&self, builder: ScreenBuilder, view: &BoardViewport) -> ScreenBuilder {
        let mut builder = builder;
        for row in [
            [
                ("board.smaller", "−", view.can_resize(false)),
                ("board.up", "Up", view.can_pan(Direction::Up)),
                ("board.larger", "+", view.can_resize(true)),
            ],
            [
                ("board.left", "Left", view.can_pan(Direction::Left)),
                ("board.down", "Down", view.can_pan(Direction::Down)),
                ("board.right", "Right", view.can_pan(Direction::Right)),
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
        let wide = metrics.width > metrics.height;
        if wide {
            return self.fitted_beside(context, probe, self.side(&metrics));
        }
        // Measured as the board's screen is laid out, which is without the
        // room the runtime leaves on a screen with no board in it. The board
        // joins that flow as one more node, so it costs one more of the gaps
        // between nodes and nothing else. Taking two Medium gaps here cost a
        // 5x5 picture its last row on a Clara BW, with a fifth of the panel
        // left empty under the controls.
        let room = |pans: bool| {
            self.status_lines()
                .into_iter()
                .map(|status| {
                    let skeleton = self
                        .play_tools(
                            Self::play_header_with(self.play_title(), status, false),
                            &probe,
                            pans,
                        )
                        .build();
                    let layout = kobo_sdk::with_room_given_up(|| {
                        skeleton.layout_with(&metrics, &Chrome::measuring(true))
                    });
                    layout.content.height - layout.flow_height - metrics.space(Space::Tight)
                })
                .min()
        };
        let mut view = self.viewport.clone().unwrap_or_else(|| probe.clone());
        let focus = if self.viewport.is_none() {
            self.focus
        } else {
            None
        };
        // Reflow only for the allocated space. Retain a panned window even when
        // its previously selected square is now outside the window. The board
        // is first given the room it has without the pan buttons, and keeps it
        // if every square shows there.
        let mut whole = view.clone();
        whole
            .reflow(
                metrics,
                metrics.content_width(),
                room(false)?,
                whole.zoom(),
                focus,
            )
            .ok()?;
        if !can_pan(&whole) {
            return Some(whole);
        }
        view.reflow(
            metrics,
            metrics.content_width(),
            room(true)?,
            view.zoom(),
            focus,
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
        let metrics = context.metrics();
        if metrics.width > metrics.height {
            return self.beside_screen(&board, &clues, &view, self.side(&metrics), &metrics);
        }
        self.play_tools(
            self.play_header(false)
                .board_viewport(&board, &clues, &view)
                .expect("matching board"),
            &view,
            can_pan(&view),
        )
        .build()
    }
    /// Held sideways, the board takes the left of the panel and its status
    /// and controls stand in a column beside it. In rows under the board
    /// they left a 5 by 5 picture no room for a row of squares on a small
    /// panel at the larger text sizes, while the side of the panel stood
    /// empty. The first arrangement whose column fits under the top bar with
    /// every status line, and with every button word on one line, is used:
    /// a button too narrow for its word broke "Right" across three lines.
    /// It is chosen for the panel and the text size alone, so panning or a
    /// larger square never moves the controls.
    fn side(&self, metrics: &kobo_sdk::DisplayMetrics) -> Side {
        let fits = |side: Side| {
            self.status_lines().into_iter().all(|status| {
                let slots: [(SlotWidth, Slot<'_>); 2] = [
                    (
                        SlotWidth::Fixed(tenths_for(metrics, side.board_width(metrics))),
                        Box::new(|slot: ScreenBuilder| slot),
                    ),
                    (
                        SlotWidth::Fill,
                        Box::new(move |slot: ScreenBuilder| {
                            self.side_tools(slot, None, true, side, status)
                        }),
                    ),
                ];
                let screen = self.play_header(true).band(BandAlign::Top, slots).build();
                let chrome = Chrome::measuring(true);
                !screen.diagnostics(metrics, &chrome).has_errors()
                    && !screen
                        .layout_with(metrics, &chrome)
                        .nodes
                        .iter()
                        .any(|node| {
                            node.text_lines.len() > 1
                                && SIDE_LABELS.contains(&node.text_lines.concat().as_str())
                        })
            })
        };
        Side::IN_ORDER
            .into_iter()
            .find(|side| fits(*side))
            .unwrap_or(Side::IN_ORDER[0])
    }
    fn fitted_beside(
        &self,
        context: &Context,
        probe: BoardViewport,
        side: Side,
    ) -> Option<BoardViewport> {
        let metrics = context.metrics();
        // The top bar is all that stands over the board.
        let skeleton = self.play_header(true).build();
        let layout = kobo_sdk::with_room_given_up(|| {
            skeleton.layout_with(&metrics, &Chrome::measuring(true))
        });
        let room = layout.content.height - layout.flow_height - metrics.space(Space::Tight);
        let mut view = self.viewport.clone().unwrap_or(probe);
        let focus = if self.viewport.is_none() {
            self.focus
        } else {
            None
        };
        let zoom = view.zoom();
        view.reflow(metrics, side.board_width(&metrics), room, zoom, focus)
            .ok()?;
        Some(view)
    }
    fn beside_screen(
        &self,
        board: &Board,
        clues: &BoardClues,
        view: &BoardViewport,
        side: Side,
        metrics: &kobo_sdk::DisplayMetrics,
    ) -> Screen {
        let pans = can_pan(view);
        let slots: [(SlotWidth, Slot<'_>); 2] = [
            (
                SlotWidth::Fixed(tenths_for(metrics, side.board_width(metrics))),
                Box::new(|slot: ScreenBuilder| {
                    slot.board_viewport(board, clues, view)
                        .expect("matching board")
                }),
            ),
            (
                SlotWidth::Fill,
                Box::new(move |slot: ScreenBuilder| {
                    self.side_tools(slot, Some(view), pans, side, self.play_status())
                }),
            ),
        ];
        self.play_header(true).band(BandAlign::Top, slots).build()
    }
    /// The status over the controls, beside the board.
    fn side_tools(
        &self,
        slot: ScreenBuilder,
        view: Option<&BoardViewport>,
        pans: bool,
        side: Side,
        status: String,
    ) -> ScreenBuilder {
        let pan = |direction| view.is_none_or(|view| view.can_pan(direction));
        let resize = |larger| view.is_none_or(|view| view.can_resize(larger));
        let left = ("board.left", "Left", pan(Direction::Left));
        let right = ("board.right", "Right", pan(Direction::Right));
        let up = ("board.up", "Up", pan(Direction::Up));
        let down = ("board.down", "Down", pan(Direction::Down));
        let smaller = ("board.smaller", "−", resize(false));
        let larger = ("board.larger", "+", resize(true));
        let undo = ("undo", "Undo", !self.undo.is_empty());
        let more = ("more", "More", true);
        let rows: Vec<Vec<(&str, &str, bool)>> = match (pans, side) {
            (false, _) => vec![vec![smaller, larger], vec![undo, more]],
            (true, Side { cross: false, .. }) => vec![
                vec![left, right],
                vec![up, down],
                vec![smaller, larger],
                vec![undo, more],
            ],
            (true, Side { cross: true, .. }) => vec![
                vec![smaller, up, larger],
                vec![left, down, right],
                vec![undo, more],
            ],
        };
        let mut builder = slot.secondary(status);
        for row in rows {
            builder = builder.band(
                BandAlign::Middle,
                row.into_iter().map(|(name, label, enabled)| {
                    (SlotWidth::Fill, move |slot: ScreenBuilder| {
                        slot.button_with_state(name, label, state(enabled))
                    })
                }),
            );
        }
        builder
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
        // Checking and run entry are settings, so they are switches that show
        // which way they are set. As buttons labelled with their state, "Run
        // off" read as an order to turn it off as easily as a report that it
        // was. How Guided does it is on the help pages: a line of summary
        // under each switch is what pushes Puzzles off a small panel held
        // sideways at the larger text sizes.
        // Undo is on the board and Back returns to it, so neither is
        // repeated here; what is left is what only this page does.
        screen
            .toggle("policy", "Guided", "", self.guided)
            .toggle("run-entry", "Run entry", "", self.run_entry)
            .button("reset", "Restart puzzle")
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
            "Send photos

On your computer, run:

kobo nonograms push IMAGE --name NAME --size N

Replace IMAGE with your image file, NAME with what to call the puzzle, and N with a grid size from 5 to 25. The puzzle appears under Photos the next time Nonograms opens or the reader wakes.

Each push adds to the photos already here. To send several at once, add --add IMAGE --name NAME --size N for each. To keep only the photos you are sending, add --replace.

Only puzzles solvable by row and column deductions are accepted. Try another photo or size if the clues need guessing.

Easy needs one solving pass, Medium two or three, and Hard more. This is a repeatable solver rating, not a prediction of your solving time."
        } else { HELP }, true)
    }
    pub(super) fn help(&self, context: &Context) -> Screen {
        let pages = self.help_pages(context);
        let page = self.help_page.min(pages.len().saturating_sub(1));
        // Add photos is its own page of instructions, and says so.
        let title = if self.route == Route::PhotoHelp {
            "Add photos"
        } else {
            "How to play"
        };
        let mut builder = ScreenBuilder::new("nonograms-help").top_bar(title);
        for paragraph in &pages[page] {
            builder = builder.text(paragraph);
        }
        // One set of page turns, at the foot with the page number between
        // them. A Previous and Next bar under them did the same twice.
        builder
            .page_turns("help-previous", "help-next")
            .page_position(
                u16::try_from(page + 1).expect("bounded help"),
                u16::try_from(pages.len().max(1)).expect("bounded help"),
            )
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
/// Whether any part of the board is off the panel, so the pan buttons have
/// somewhere to take it.
/// One column of a band, built when the band is laid out.
type Slot<'a> = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder + 'a>;

/// The words on the buttons beside a board held sideways.
const SIDE_LABELS: [&str; 8] = ["Left", "Right", "Up", "Down", "−", "+", "Undo", "More"];

/// How the controls stand beside a board held sideways.
#[derive(Clone, Copy)]
struct Side {
    /// The board's share of the width, in tenths.
    tenths: i32,
    /// The pan buttons in a cross of three rather than in pairs.
    cross: bool,
}
impl Side {
    /// Widest board first. Pairs need the least width per button, so they
    /// come before a cross at each share.
    const IN_ORDER: [Self; 6] = [
        Self {
            tenths: 6,
            cross: false,
        },
        Self {
            tenths: 6,
            cross: true,
        },
        Self {
            tenths: 5,
            cross: false,
        },
        Self {
            tenths: 5,
            cross: true,
        },
        Self {
            tenths: 4,
            cross: false,
        },
        Self {
            tenths: 4,
            cross: true,
        },
    ];
    fn board_width(self, metrics: &kobo_sdk::DisplayMetrics) -> i32 {
        metrics.content_width() * self.tenths / 10
    }
}

/// The fewest tenths of a millimetre that are at least `pixels` on this
/// panel, so a slot of that width holds a board of that width whole.
fn tenths_for(metrics: &kobo_sdk::DisplayMetrics, pixels: i32) -> u16 {
    let mut tenths = pixels.saturating_mul(254) / metrics.pixels_per_inch.max(1);
    while metrics.tenth_mm(tenths) < pixels {
        tenths += 1;
    }
    u16::try_from(tenths).unwrap_or(u16::MAX)
}

fn can_pan(view: &BoardViewport) -> bool {
    [
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ]
    .into_iter()
    .any(|direction| view.can_pan(direction))
}
