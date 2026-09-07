//! Inkling: a deterministic, offline five-letter daily puzzle.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};
const SALT: &str = "inkling-offline-2026";
const ANSWERS: &[&str] = &[
    "crane", "stare", "piano", "flint", "woven", "mirth", "caper", "bloom", "quiet", "ridge",
    "slope", "charm",
];
const GUESSES: &[&str] = &[
    "adore", "alert", "alien", "alone", "amber", "ample", "apple", "beach", "beard", "berry",
    "black", "blade", "bread", "brick", "bring", "brown", "chair", "chase", "chime", "clean",
    "clear", "climb", "clock", "cloud", "coral", "dance", "dream", "earth", "field", "flame",
    "fresh", "front", "giant", "glass", "grape", "green", "heart", "house", "ivory", "jolly",
    "kneel", "lemon", "light", "maple", "metal", "night", "ocean", "olive", "pearl", "plant",
    "proud", "river", "roast", "round", "shine", "shore", "smart", "smile", "sound", "spice",
    "stone", "sugar", "table", "tiger", "toast", "train", "water", "whale", "wheat", "world",
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
    ANSWERS.contains(&word) || GUESSES.contains(&word)
}
fn civil_date(days_since_epoch: i64) -> String {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() / 86_400);
    civil_date(i64::try_from(days).unwrap_or(i64::MAX))
}
fn hard_allows(answer: &str, prior: &[String], guess: &str) -> bool {
    let guess_bytes = guess.as_bytes();
    let mut required = [0_u8; 26];
    for old in prior {
        let old_bytes = old.as_bytes();
        let scored = marks(answer, old);
        let mut seen = [0_u8; 26];
        for (index, mark) in scored.into_iter().enumerate() {
            let letter = old_bytes[index];
            if mark == Mark::Placed && guess_bytes[index] != letter {
                return false;
            }
            if mark == Mark::Present && guess_bytes[index] == letter {
                return false;
            }
            if mark != Mark::Absent {
                let offset = usize::from(letter.saturating_sub(b'a'));
                if offset < seen.len() {
                    seen[offset] = seen[offset].saturating_add(1);
                }
            }
        }
        for (need, count) in required.iter_mut().zip(seen) {
            *need = (*need).max(count);
        }
    }
    required.iter().enumerate().all(|(offset, needed)| {
        let letter = b'a' + u8::try_from(offset).expect("alphabet index");
        guess_bytes.iter().fold(0_usize, |count, candidate| {
            count + usize::from(*candidate == letter)
        }) >= usize::from(*needed)
    })
}
struct Game {
    date: String,
    answer: &'static str,
    guesses: Vec<String>,
    keyboard: Keyboard,
    notice: String,
    hard: bool,
    typing: bool,
    help: bool,
}
impl Default for Game {
    fn default() -> Self {
        let date = std::env::var("KOBO_INKLING_DAY").unwrap_or_else(|_| today());
        Self {
            answer: answer_for(&date),
            date,
            guesses: Vec::new(),
            keyboard: Keyboard::new(),
            notice: "Six guesses. Shape states do not rely on color.".into(),
            hard: false,
            typing: false,
            help: false,
        }
    }
}
impl Game {
    fn done(&self) -> bool {
        self.guesses.len() >= 6
            || self
                .guesses
                .last()
                .is_some_and(|guess| guess == self.answer)
    }

    fn submit(&mut self) {
        let guess = self.keyboard.take().to_ascii_lowercase();
        if guess.len() != 5 {
            self.notice = "Use five letters.".into();
            return;
        }
        if !valid(&guess) {
            self.notice = "Not in the word list.".into();
            return;
        }
        if self.hard && !hard_allows(self.answer, &self.guesses, &guess) {
            self.notice = "Hard mode requires every revealed letter and position.".into();
            return;
        }
        self.guesses.push(guess.clone());
        self.notice = if guess == self.answer {
            "Solved.".into()
        } else if self.done() {
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
        if self.help {
            return ScreenBuilder::new("inkling-help")
                .top_bar("How to play")
                .owns_back(true)
                .heading("Find the five-letter word")
                .text("You have six guesses. Type five letters, then tap Guess.")
                .text("[A] is in the right spot. (A) is elsewhere in the word. A× is absent.")
                .text("Hard mode makes you reuse letters already placed correctly.")
                .bottom_action("close-help", "Play")
                .build();
        }
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
                ("how-to-play", "How to play"),
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
        if self.help {
            if a == action_id("close-help") || a == ActionId::BACK {
                self.help = false;
                changed = true;
            }
        } else if a == action_id("how-to-play") {
            self.help = true;
            changed = true;
        } else if self.typing {
            if let Some(p) = self.keyboard.press(a) {
                changed = true;
                if p == Pressed::Submitted && !self.done() {
                    self.submit();
                    self.typing = false;
                }
            } else if a == action_id("cancel") {
                self.typing = false;
                changed = true;
            }
        } else if a == action_id("enter") && !self.done() {
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
                usize::from(self.done()),
                usize::from(self.done() && self.guesses.last().is_some_and(|g| g == self.answer))
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
    fn unix_days_format_as_real_calendar_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_697), "2026-09-01");
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
    fn hard_mode_reuses_present_letters_and_fixed_positions() {
        let prior = vec!["crane".to_owned()];
        assert!(hard_allows("caper", &prior, "cater"));
        assert!(!hard_allows("caper", &prior, "slope"));
        assert!(!hard_allows("caper", &prior, "crown"));
    }
    #[test]
    fn clara_layout_is_clean() {
        let s = Game::default().screen();
        let d = s.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(d.issues.is_empty(), "{:?}", d.issues);
    }
    #[test]
    fn how_to_play_is_short_and_reachable() {
        let mut game = Game::default();
        let home = game.screen();
        assert!(home
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("how-to-play"))
            .is_some());
        game.help = true;
        let help = game.screen();
        assert!(help
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }
}
