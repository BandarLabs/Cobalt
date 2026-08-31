//! A small, bundled, line-solvable nonogram for e-ink readers.
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

const SIDE: usize = 9;
const ANSWER: [bool; SIDE * SIDE] = [
    false, false, true, true, true, true, true, true, false,
    false, true, false, false, false, false, false, false, true,
    true, false, true, false, false, false, false, true, false,
    true, false, false, true, false, false, true, false, false,
    true, false, false, false, true, true, false, false, false,
    true, false, false, false, true, true, false, false, false,
    true, false, false, true, false, false, true, false, false,
    true, false, true, false, false, false, false, true, false,
    false, true, false, false, false, false, false, false, true,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mark { Blank, Fill, Cross }

impl Mark {
    const fn next(self) -> Self {
        match self { Self::Blank => Self::Fill, Self::Fill => Self::Cross, Self::Cross => Self::Blank }
    }
    const fn label(self) -> &'static str {
        match self { Self::Blank => " ", Self::Fill => "■", Self::Cross => "×" }
    }
}

struct Game { marks: [Mark; SIDE * SIDE], guided: bool, done: bool }

impl Default for Game {
    fn default() -> Self { Self { marks: [Mark::Blank; SIDE * SIDE], guided: false, done: false } }
}

fn runs(line: impl IntoIterator<Item = bool>) -> String {
    let mut out = Vec::new(); let mut count = 0;
    for filled in line {
        if filled { count += 1; } else if count > 0 { out.push(count); count = 0; }
    }
    if count > 0 { out.push(count); }
    out.into_iter().map(|count| count.to_string()).collect::<Vec<_>>().join(" ")
}
fn cell_name(cell: usize) -> String { format!("cell-{cell}") }

impl Game {
    fn toggle(&mut self, cell: usize) -> bool {
        if cell >= SIDE * SIDE || self.done { return false; }
        self.marks[cell] = self.marks[cell].next();
        if self.marks.iter().enumerate().all(|(i, mark)| (*mark == Mark::Fill) == ANSWER[i]) {
            self.done = true;
        }
        true
    }
    fn status(&self) -> String {
        if self.done { "Solved. The drawing is complete.".into() }
        else if self.guided { "Guided marks: tap a square to cycle fill, X, blank.".into() }
        else { "Free marks: tap a square to cycle fill, X, blank.".into() }
    }
}

fn screen(game: &Game) -> Screen {
    let cells = (0..SIDE * SIDE).map(|cell| (cell_name(cell), format!("{cell:02} {}", game.marks[cell].label()), None));
    ScreenBuilder::new("nonograms")
        .top_bar("Nonograms").secondary(game.status())
        .secondary("Rows: 6 / 1 1 / 1 1 1 / 1 1 1 / 1 2 1 1")
        .board(SIDE as u8, cells)
        .grid(2, false, [("policy", if game.guided { "Guided" } else { "Free" }), ("reset", "Reset")])
        .build()
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) { context.set_screen(screen(self)); }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let changed = if action == action_id("policy") { self.guided = !self.guided; true }
        else if action == action_id("reset") { *self = Self::default(); true }
        else if let Some(cell) = (0..SIDE * SIDE).find(|cell| action == action_id(&cell_name(*cell))) { self.toggle(cell) }
        else { false };
        if changed { context.set_screen(screen(self)); }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("nonograms", Game::default()) { Ok(()) => ExitCode::SUCCESS, Err(error) => { eprintln!("nonograms: {error}"); ExitCode::FAILURE } }
}

#[cfg(test)]
mod tests {
    use super::*; use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test] fn run_clues_describe_lines() { assert_eq!(runs([false, true, true, false, true]), "2 1"); assert_eq!(runs([false, false]), ""); }
    #[test] fn marks_cycle_and_completion_requires_exact_picture() {
        let mut game = Game::default(); assert!(game.toggle(0)); assert_eq!(game.marks[0], Mark::Fill); assert!(game.toggle(0)); assert_eq!(game.marks[0], Mark::Cross);
        for (i, answer) in ANSWER.iter().enumerate() { game.marks[i] = if *answer { Mark::Fill } else { Mark::Cross }; } game.done = false; assert!(game.toggle(0)); assert!(game.done);
    }
    #[test] fn smallest_panel_has_reachable_square_cells() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for cell in 0..SIDE * SIDE { let rect = layout.rect_of_action(action_id(&cell_name(cell))).expect("cell"); assert_eq!(rect.width, rect.height); assert!(rect.width >= CLARA_BW_METRICS.touch_target_minimum()); }
        assert!(screen(&Game::default()).diagnostics(&CLARA_BW_METRICS, &Chrome::default()).issues.is_empty());
    }
}
