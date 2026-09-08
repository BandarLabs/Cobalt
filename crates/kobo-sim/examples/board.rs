//! Original board fixture through SDK IPC. Pass a socket in a private directory.
use kobo_sdk::{
    action_id,
    board::{Board, BoardClues, BoardViewport, Field, Mark},
    ActionId, Context, KoboApp, ScreenBuilder,
};

struct Study {
    board: Board,
    clues: BoardClues,
    view: Option<BoardViewport>,
    detail: Option<(String, String)>,
}
impl Study {
    fn new() -> Self {
        let marks = [
            Mark::Empty,
            Mark::Filled,
            Mark::Crossed,
            Mark::Dot,
            Mark::Value(7),
            Mark::Notes(5),
        ];
        Self {
            board: Board::new(
                64,
                64,
                (0..4096).map(|index| Field::editable(marks[index % marks.len()])),
            )
            .unwrap(),
            clues: BoardClues {
                rows: vec![vec![1, 2, 3, 4, 5]; 64],
                columns: vec![vec![6, 1, 2, 3]; 64],
            },
            view: None,
            detail: None,
        }
    }
    fn show(&mut self, context: &mut Context) {
        let metrics = context.metrics();
        if self.view.is_none() {
            self.view = Some(
                BoardViewport::new(
                    &self.board,
                    &self.clues,
                    metrics,
                    metrics.content_width(),
                    metrics.height / 3,
                )
                .unwrap(),
            );
        }
        let view = self.view.as_ref().unwrap();
        let builder = if let Some((title, text)) = &self.detail {
            ScreenBuilder::new("board-clue")
                .top_bar(title)
                .text(text)
                .button("close", "Close")
        } else {
            ScreenBuilder::new(format!(
                "board-study-{}-{:?}",
                view.position(),
                view.zoom()
            ))
            .top_bar("Board study")
            .secondary(view.position())
            .board_viewport(&self.board, &self.clues, view)
            .unwrap()
            .board_viewport_controls(view)
        };
        context.set_screen(
            builder
                .build_checked_with(&metrics, &kobo_sdk::Chrome::with_back(true))
                .unwrap()
                .with_own_back(true),
        );
    }
}
impl KoboApp for Study {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK || action == action_id("close") {
            self.detail = None;
            self.show(context);
            return;
        }
        let view = self.view.as_mut().unwrap();
        if self.detail.is_some() {
            return;
        }
        let selected = view
            .cells()
            .find(|cell| action_id(&format!("board.cell.{cell}")) == action);
        if let Some(cell) = selected {
            self.board.select(cell).unwrap();
        }
        for (axis, range) in [
            ("row", view.visible_rows()),
            ("column", view.visible_columns()),
        ] {
            for index in range {
                let name = format!("board.{axis}.{index}");
                if action_id(&name) == action {
                    self.detail = view.inspect_clue(&name, &self.clues);
                }
            }
        }
        for name in [
            "board.left",
            "board.right",
            "board.up",
            "board.down",
            "board.smaller",
            "board.larger",
        ] {
            if action_id(name) == action {
                view.navigate(name, self.board.selected()).unwrap();
            }
        }
        self.show(context);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Pass a socket path in a private directory.")?,
    );
    let server = kobo_sim::AppServer::bind("127.0.0.1:0", &socket)?;
    println!("Board simulator: http://{}", server.local_addr()?);
    let _app = std::thread::spawn(move || {
        if let Err(error) = kobo_sdk::run_on("board-study", Study::new(), &socket) {
            eprintln!("{error}");
        }
    });
    server.serve()?;
    Ok(())
}
