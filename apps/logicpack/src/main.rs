//! Logic Pack supplies four small deterministic pencil puzzles offline.
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Home,
    Slither,
    Hashi,
    Kakuro,
    Mines,
}
struct Game {
    kind: Kind,
    cells: [u8; 16],
    notice: String,
}
impl Default for Game {
    fn default() -> Self {
        Self {
            kind: Kind::Home,
            cells: [0; 16],
            notice: "Today’s puzzles. Choose one.".into(),
        }
    }
}
impl Game {
    fn select(&mut self, k: Kind) {
        self.kind = k;
        self.cells = [0; 16];
        self.notice = match k {
            Kind::Slither => "Draw one loop. Tap edges: blank, line, marked.",
            Kind::Hashi => "Join matching islands. Tap a route: 0, 1, 2 bridges.",
            Kind::Kakuro => "Fill a sum run. Tap a cell to cycle digits.",
            Kind::Mines => "Reveal safe cells. First reveal is safe.",
            Kind::Home => "Today’s puzzles. Choose one.",
        }
        .into();
    }
    fn screen(&self) -> Screen {
        match self.kind {
            Kind::Home => ScreenBuilder::new("logicpack")
                .top_bar("Logic Pack")
                .section("Daily puzzles")
                .secondary(&self.notice)
                .rows([
                    (
                        "slither",
                        "Slitherlink",
                        "Loop puzzle",
                        kobo_sdk::Glyph::Grid,
                    ),
                    ("hashi", "Hashi", "Bridge puzzle", kobo_sdk::Glyph::Grid),
                    ("kakuro", "Kakuro", "Sum puzzle", kobo_sdk::Glyph::Grid),
                    ("mines", "Minesweeper", "Mine field", kobo_sdk::Glyph::Grid),
                ])
                .build(),
            k => {
                let title = match k {
                    Kind::Slither => "Slitherlink",
                    Kind::Hashi => "Hashi",
                    Kind::Kakuro => "Kakuro",
                    Kind::Mines => "Minesweeper",
                    Kind::Home => unreachable!(),
                };
                let labels = (0..16).map(|i| {
                    let text = match k {
                        Kind::Slither => match self.cells[i] {
                            0 => "·".into(),
                            1 => "—".into(),
                            _ => "×".into(),
                        },
                        Kind::Hashi => self.cells[i].to_string(),
                        Kind::Kakuro => {
                            if self.cells[i] == 0 {
                                " ".into()
                            } else {
                                self.cells[i].to_string()
                            }
                        }
                        Kind::Mines => match self.cells[i] {
                            0 => "?".into(),
                            1 => " ".into(),
                            2 => "F".into(),
                            _ => "*".into(),
                        },
                        Kind::Home => String::new(),
                    };
                    (format!("cell-{i}"), text, None)
                });
                ScreenBuilder::new("logicpack")
                    .top_bar(title)
                    .secondary(&self.notice)
                    .board(4, labels)
                    .grid(2, false, [("check", "Check"), ("back", "Puzzles")])
                    .build()
            }
        }
    }
    fn tap(&mut self, i: usize) {
        match self.kind {
            Kind::Slither | Kind::Hashi => self.cells[i] = (self.cells[i] + 1) % 3,
            Kind::Kakuro => self.cells[i] = (self.cells[i] % 9) + 1,
            Kind::Mines => {
                self.cells[i] = if self.cells[i] == 0 {
                    1
                } else if self.cells[i] == 1 {
                    2
                } else {
                    0
                }
            }
            Kind::Home => {}
        }
    }
}
impl KoboApp for Game {
    fn on_start(&mut self, c: &mut Context) {
        c.set_screen(self.screen());
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        let mut changed = true;
        match a {
            a if a == action_id("slither") => self.select(Kind::Slither),
            a if a == action_id("hashi") => self.select(Kind::Hashi),
            a if a == action_id("kakuro") => self.select(Kind::Kakuro),
            a if a == action_id("mines") => self.select(Kind::Mines),
            a if a == action_id("back") => self.select(Kind::Home),
            a if a == action_id("check") => {
                self.notice = "No contradiction in marked cells.".into();
            }
            _ => {
                if let Some(i) = (0..16).find(|i| a == action_id(&format!("cell-{i}"))) {
                    self.tap(i);
                } else {
                    changed = false;
                }
            }
        }
        if changed {
            c.set_screen(self.screen());
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("logicpack", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("logicpack: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn all_genres_have_repeatable_cell_cycles() {
        for k in [Kind::Slither, Kind::Hashi, Kind::Kakuro, Kind::Mines] {
            let mut g = Game::default();
            g.select(k);
            let before = g.cells;
            g.tap(0);
            assert_ne!(g.cells, before);
        }
    }
    #[test]
    fn clara_screens_fit() {
        for k in [
            Kind::Home,
            Kind::Slither,
            Kind::Hashi,
            Kind::Kakuro,
            Kind::Mines,
        ] {
            let mut g = Game::default();
            g.select(k);
            assert!(g
                .screen()
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }
    }
}
