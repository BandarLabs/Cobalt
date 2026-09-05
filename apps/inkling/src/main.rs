//! Inkling: a deterministic, offline five-letter daily puzzle.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;
const SALT: &str = "inkling-offline-2026";
const ANSWERS: &[&str] = &[
    "crane", "stare", "piano", "flint", "woven", "mirth", "caper", "bloom", "quiet", "ridge",
    "slope", "charm",
];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mark {
    Absent,
    Present,
    Placed,
}
fn djb2(s: &str) -> u64 {
    s.bytes().fold(5381_u64, |h, b| {
        h.wrapping_mul(33).wrapping_add(u64::from(b))
    })
}
fn answer_for(date: &str) -> &'static str {
    let answer_count = u64::try_from(ANSWERS.len()).expect("the answer list fits u64");
    let index = usize::try_from(djb2(&format!("{date}{SALT}")) % answer_count)
        .expect("the reduced answer index fits usize");
    ANSWERS[index]
}
fn marks(answer: &str, guess: &str) -> [Mark; 5] {
    let mut out = [Mark::Absent; 5];
    let a = answer.as_bytes();
    let g = guess.as_bytes();
    let mut used = [false; 5];
    for i in 0..5 {
        if g[i] == a[i] {
            out[i] = Mark::Placed;
            used[i] = true;
        }
    }
    for i in 0..5 {
        if out[i] != Mark::Placed {
            if let Some(j) = (0..5).find(|&j| !used[j] && a[j] == g[i]) {
                out[i] = Mark::Present;
                used[j] = true;
            }
        }
    }
    out
}
fn valid(word: &str) -> bool {
    ANSWERS.contains(&word)
}
struct Game {
    date: String,
    answer: &'static str,
    guesses: Vec<String>,
    keyboard: Keyboard,
    notice: String,
    hard: bool,
    done: bool,
    typing: bool,
}
impl Default for Game {
    fn default() -> Self {
        let date = "2026-09-01".to_owned();
        Self {
            answer: answer_for(&date),
            date,
            guesses: Vec::new(),
            keyboard: Keyboard::new(),
            notice: "Six guesses. Shape states do not rely on color.".into(),
            hard: false,
            done: false,
            typing: false,
        }
    }
}
impl Game {
    fn submit(&mut self) {
        let guess = self.keyboard.take().to_ascii_lowercase();
        if guess.len() != 5 {
            self.notice = "Use five letters.".into();
            return;
        }
        if !valid(&guess) {
            self.notice = "not in the dictionary".into();
            return;
        }
        if self.hard && !self.guesses.is_empty() {
            let prior = marks(self.answer, &self.guesses[0]);
            for (i, m) in prior.iter().enumerate() {
                if *m == Mark::Placed && guess.as_bytes()[i] != self.guesses[0].as_bytes()[i] {
                    self.notice = "Hard mode requires placed letters.".into();
                    return;
                }
            }
        }
        self.guesses.push(guess.clone());
        self.done = guess == self.answer || self.guesses.len() == 6;
        self.notice = if guess == self.answer {
            "Solved.".into()
        } else if self.done {
            format!("Answer: {}", self.answer)
        } else {
            format!("{} of 6", self.guesses.len())
        };
    }
    fn cell(&self, row: usize, col: usize) -> String {
        if let Some(g) = self.guesses.get(row) {
            let c = g.chars().nth(col).unwrap_or(' ');
            match marks(self.answer, g)[col] {
                Mark::Placed => format!("[{c}]"),
                Mark::Present => format!("({c})"),
                Mark::Absent => format!("{c}×"),
            }
        } else {
            " ".into()
        }
    }
    fn screen(&self) -> Screen {
        if self.typing {
            return ScreenBuilder::new("inkling")
                .top_bar("Inkling")
                .typed(&self.keyboard, "Type five letters")
                .keyboard(&self.keyboard, "Guess")
                .bottom_action("cancel", "Cancel")
                .build();
        }
        let cells = (0..30).map(|i| (format!("cell-{i}"), self.cell(i / 5, i % 5)));
        ScreenBuilder::new("inkling")
            .top_bar(format!("Inkling  #{}", djb2(&self.date) % 10_000))
            .secondary(&self.notice)
            .grid(5, false, cells)
            .button("enter", "Enter guess")
            .action_bar([
                (
                    "hard",
                    if self.hard {
                        "Hard mode on"
                    } else {
                        "Hard mode off"
                    },
                ),
                ("stats", "Stats"),
            ])
            .build()
    }
}
impl KoboApp for Game {
    fn on_start(&mut self, c: &mut Context) {
        c.set_screen(self.screen());
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        let mut changed = false;
        if self.typing {
            if let Some(p) = self.keyboard.press(a) {
                changed = true;
                if p == Pressed::Submitted && !self.done {
                    self.submit();
                    self.typing = false;
                }
            } else if a == action_id("cancel") {
                self.typing = false;
                changed = true;
            }
        } else if a == action_id("enter") {
            self.typing = true;
            changed = true;
        } else if a == action_id("hard") {
            self.hard = !self.hard;
            self.notice = if self.hard {
                "Hard mode on."
            } else {
                "Hard mode off."
            }
            .into();
            changed = true;
        } else if a == action_id("stats") {
            self.notice = format!(
                "Played {}. Wins {}.",
                usize::from(self.done),
                usize::from(self.done && self.guesses.last().is_some_and(|g| g == self.answer))
            );
            changed = true;
        }
        if changed {
            c.set_screen(self.screen());
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("inkling", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inkling: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn dates_are_deterministic() {
        for y in 2020..2031 {
            let d = format!("{y}-09-01");
            assert_eq!(answer_for(&d), answer_for(&d));
        }
    }
    #[test]
    fn duplicate_letters_are_scored_once() {
        assert_eq!(
            marks("bloom", "ooooo"),
            [
                Mark::Absent,
                Mark::Absent,
                Mark::Placed,
                Mark::Placed,
                Mark::Absent
            ]
        );
    }
    #[test]
    fn answers_are_valid_words() {
        assert!(ANSWERS.iter().all(|w| valid(w) && w.len() == 5));
    }
    #[test]
    fn clara_layout_is_clean() {
        let s = Game::default().screen();
        let d = s.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }
}
