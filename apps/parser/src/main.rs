//! Parser is a compact, offline, touch-first interactive fiction player.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Room {
    Study,
    Garden,
}
struct Game {
    room: Room,
    lamp: bool,
    score: u8,
    turns: u16,
    transcript: Vec<String>,
    keyboard: Keyboard,
}
impl Default for Game {
    fn default() -> Self {
        Self {
            room: Room::Study,
            lamp: false,
            score: 0,
            turns: 0,
            transcript: vec![
                "The study is quiet. A brass lamp waits on the desk. The door leads north.".into(),
            ],
            keyboard: Keyboard::new(),
        }
    }
}
impl Game {
    fn command(&mut self, input: &str) {
        let word = input.trim().to_ascii_lowercase();
        if word.is_empty() {
            return;
        }
        self.turns += 1;
        let reply = match word.as_str() {
            "look" | "l" => match self.room {
                Room::Study => "The lamp is unlit. The north door is open.",
                Room::Garden => "Moonlight shows a gate and a stone bench.",
            },
            "inventory" | "i" => {
                if self.lamp {
                    "You carry the lit lamp."
                } else {
                    "Your hands are empty."
                }
            }
            "take lamp" | "take" => {
                self.lamp = true;
                self.score = 1;
                "Taken."
            }
            "north" | "n" | "go north" => {
                self.room = Room::Garden;
                "You step into the garden."
            }
            "south" | "s" | "go south" => {
                self.room = Room::Study;
                "You return to the study."
            }
            "examine lamp" => "A brass lamp. It has no switch; taking it wakes its flame.",
            _ => "I do not understand that command.",
        };
        self.transcript.push(format!("> {input}"));
        self.transcript.push(reply.into());
        if self.transcript.len() > 8 {
            self.transcript.drain(0..2);
        }
    }
    fn helper(&mut self, s: &str) {
        self.command(s)
    }
    fn screen(&self) -> Screen {
        let lines = self.transcript.join("\n\n");
        ScreenBuilder::new("parser")
            .top_bar(format!(
                "Parser   Score {}  Turns {}",
                self.score, self.turns
            ))
            .reading(true)
            .text(lines)
            .divider()
            .typed(&self.keyboard, "Type a command")
            .keyboard(&self.keyboard, "Run")
            .grid(
                3,
                false,
                [
                    ("look", "LOOK"),
                    ("inventory", "INVENTORY"),
                    ("take", "TAKE"),
                    ("north", "NORTH"),
                    ("south", "SOUTH"),
                    ("examine", "EXAMINE LAMP"),
                ],
            )
            .build()
    }
}
impl KoboApp for Game {
    fn on_start(&mut self, c: &mut Context) {
        c.set_screen(self.screen())
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        let mut changed = false;
        if let Some(p) = self.keyboard.press(a) {
            changed = true;
            if p == Pressed::Submitted {
                let input = self.keyboard.take();
                self.command(&input)
            }
        } else {
            for (n, v) in [
                ("look", "look"),
                ("inventory", "inventory"),
                ("take", "take lamp"),
                ("north", "north"),
                ("south", "south"),
                ("examine", "examine lamp"),
            ] {
                if a == action_id(n) {
                    self.helper(v);
                    changed = true;
                }
            }
        }
        if changed {
            c.set_screen(self.screen())
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("parser", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parser: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn commands_reach_a_playable_ending_state() {
        let mut g = Game::default();
        g.command("take lamp");
        g.command("north");
        assert!(g.lamp);
        assert_eq!(g.room, Room::Garden);
        assert_eq!(g.score, 1);
    }
    #[test]
    fn clara_layout_fits() {
        let s = Game::default().screen();
        let d = s.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        assert!(s
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("look"))
            .is_some());
    }
}
