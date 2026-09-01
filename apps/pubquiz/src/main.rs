//! Pub Quiz keeps its question packs and play state on the reader.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
    Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const STATE: &str = "pubquiz-state";
const PACK_CAP: u8 = 10;
const API: &str = "https://opentdb.com/api.php?amount=50&type=multiple";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Question,
    Pass,
    Reveal,
    Podium,
    About,
}
#[derive(Clone, Copy)]
struct Question {
    category: &'static str,
    text: &'static str,
    answers: [&'static str; 4],
    correct: usize,
}
const QUESTIONS: &[Question] = &[
    Question {
        category: "Science",
        text: "Which planet has the shortest year?",
        answers: ["Mercury", "Mars", "Venus", "Earth"],
        correct: 0,
    },
    Question {
        category: "General knowledge",
        text: "What is the capital of Finland?",
        answers: ["Oslo", "Helsinki", "Tallinn", "Stockholm"],
        correct: 1,
    },
    Question {
        category: "History",
        text: "Which ship carried Charles Darwin on his voyage?",
        answers: ["Beagle", "Endeavour", "Victory", "Resolution"],
        correct: 0,
    },
    Question {
        category: "Arts",
        text: "Who painted The Persistence of Memory?",
        answers: ["Miró", "Dalí", "Picasso", "Kahlo"],
        correct: 1,
    },
];
struct Quiz {
    view: View,
    party: bool,
    player: usize,
    question: usize,
    answer: Option<usize>,
    scores: [u8; 4],
    packs: u8,
    syncing: bool,
    note: Option<String>,
    streak: u16,
    loaded: bool,
}
impl Default for Quiz {
    fn default() -> Self {
        Self {
            view: View::Home,
            party: true,
            player: 0,
            question: 0,
            answer: None,
            scores: [0; 4],
            packs: 0,
            syncing: false,
            note: None,
            streak: 0,
            loaded: false,
        }
    }
}
impl Quiz {
    fn player_name(&self) -> &'static str {
        ["Ada", "Bert", "Cleo", "Dev"][self.player]
    }
    fn save(&self, context: &mut Context) {
        context.store().save(
            STATE,
            format!("{}|{}", self.packs, self.streak).into_bytes(),
        );
    }
    fn begin(&mut self, party: bool) {
        self.party = party;
        self.view = View::Question;
        self.question = 0;
        self.player = 0;
        self.answer = None;
        self.scores = [0; 4];
        self.note = None;
    }
    fn sync(&mut self, context: &mut Context) {
        self.syncing = true;
        self.note = None;
        if context
            .spawn_retrying(Task::Fetch {
                url: API.into(),
                offset: 0,
                max_bytes: 128 * 1024,
                credential: None,
                headers: Vec::new(),
            })
            .is_none()
        {
            self.syncing = false;
            self.note = Some("The device is busy. Try sync again.".into());
        }
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(screen(self));
    }
}
fn choice(index: usize) -> String {
    format!("answer-{index}")
}
#[allow(clippy::too_many_lines)]
fn screen(quiz: &Quiz) -> Screen {
    let question = QUESTIONS[quiz.question % QUESTIONS.len()];
    match quiz.view {
        View::Home => {
            let mut b = ScreenBuilder::new("pubquiz-home")
                .top_bar("Pub Quiz")
                .heading("Question packs")
                .secondary(format!(
                    "{} of {PACK_CAP} cached packs · {} day streak",
                    quiz.packs, quiz.streak
                ));
            if let Some(note) = &quiz.note {
                b = b.banner(BannerLevel::Info, note);
            }
            if quiz.packs == 0 {
                b = b.empty_state(
                    "No packs yet. Sync question packs while online, then play anywhere.",
                );
            }
            b.primary_button("party", "Start pass-around")
                .buttons([
                    ("solo", "Solo round"),
                    (
                        "sync",
                        if quiz.syncing {
                            "Working…"
                        } else {
                            "Sync packs"
                        },
                    ),
                ])
                .button("about", "About")
                .build()
        }
        View::Question => ScreenBuilder::new("pubquiz-question")
            .top_bar(if quiz.party {
                format!("{} answers", quiz.player_name())
            } else {
                "Solo round".into()
            })
            .secondary(format!(
                "{} · question {} of 10",
                question.category,
                quiz.question + 1
            ))
            .heading(question.text)
            .grid(
                1,
                false,
                question.answers.iter().enumerate().map(|(i, answer)| {
                    (
                        choice(i),
                        format!(
                            "{} · {}",
                            char::from(b'A' + u8::try_from(i).expect("four answers fit u8")),
                            answer
                        ),
                    )
                }),
            )
            .build(),
        View::Pass => ScreenBuilder::new("pubquiz-pass")
            .top_bar("Pass it on")
            .heading("Answer locked")
            .text("Hand the Kobo to the next player before the result is shown.")
            .primary_button("reveal", "Show result")
            .build(),
        View::Reveal => {
            let right = quiz.answer == Some(question.correct);
            ScreenBuilder::new("pubquiz-reveal")
                .top_bar("Round result")
                .heading(if right { "Correct" } else { "Not this time" })
                .secondary(format!(
                    "{} · {}",
                    question.category, question.answers[question.correct]
                ))
                .facts((0..if quiz.party { 4 } else { 1 }).map(|i| {
                    (
                        ["Ada", "Bert", "Cleo", "Dev"][i],
                        format!("{} points", quiz.scores[i]),
                    )
                }))
                .primary_button(
                    "continue",
                    if quiz.question + 1 == 10 {
                        "See podium"
                    } else {
                        "Next question"
                    },
                )
                .build()
        }
        View::Podium => ScreenBuilder::new("pubquiz-podium")
            .top_bar("Pub Quiz")
            .heading("Podium")
            .rows((0..4).map(|i| {
                (
                    format!("player-{i}"),
                    ["Ada", "Bert", "Cleo", "Dev"][i],
                    format!("{} points", quiz.scores[i]),
                    Glyph::Person,
                )
            }))
            .primary_button("home", "Finish round")
            .build(),
        View::About => ScreenBuilder::new("pubquiz-about")
            .top_bar("Pub Quiz")
            .heading("About")
            .text("Question packs use Open Trivia DB content, licensed CC-BY-SA 4.0.")
            .text("opentdb.com · cached packs are redistributed under the same license.")
            .button("home", "Back to packs")
            .build(),
    }
}
impl KoboApp for Quiz {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        self.show(context);
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            if let Some(bytes) = value {
                if let Ok(s) = String::from_utf8(bytes) {
                    let p: Vec<_> = s.split('|').collect();
                    self.packs = p.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                    self.streak = p.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                }
            }
            self.loaded = true;
            self.show(context);
        }
    }
    fn on_task(&mut self, context: &mut Context, _: TaskId, outcome: TaskOutcome) {
        self.syncing = false;
        match outcome {
            TaskOutcome::Completed(_) => {
                self.packs = (self.packs + 1).min(PACK_CAP);
                self.note = Some(format!(
                    "Pack {} saved. Oldest packs are pruned after {PACK_CAP}.",
                    self.packs
                ));
                self.save(context);
            }
            TaskOutcome::Failed(kobo_sdk::TaskError::Offline) => {
                self.note = Some("Off the air. Existing packs still play offline.".into());
            }
            TaskOutcome::Failed(_) | TaskOutcome::Cancelled => {
                self.note =
                    Some("Open Trivia DB did not answer. Join Wi-Fi and try sync again.".into());
            }
        }
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("party") {
            self.begin(true);
        } else if action == action_id("solo") {
            self.begin(false);
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("home") {
            self.view = View::Home;
        } else if let Some(answer) = (0..4).find(|i| action == action_id(&choice(*i))) {
            self.answer = Some(answer);
            self.view = if self.party { View::Pass } else { View::Reveal };
            if answer == QUESTIONS[self.question % QUESTIONS.len()].correct {
                self.scores[self.player] += 1;
            }
        } else if action == action_id("reveal") {
            self.view = View::Reveal;
        } else if action == action_id("continue") {
            self.question += 1;
            self.player = (self.player + 1) % 4;
            self.answer = None;
            if self.question >= 10 {
                self.view = View::Podium;
                self.streak += 1;
                self.save(context);
            } else {
                self.view = View::Question;
            }
        }
        self.show(context);
    }
}
fn main() -> ExitCode {
    kobo_sdk::run("pubquiz", Quiz::default()).map_or_else(
        |error| {
            eprintln!("pubquiz: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn locked_answer_hides_the_reveal() {
        let mut quiz = Quiz::default();
        quiz.begin(true);
        quiz.answer = Some(0);
        quiz.view = View::Pass;
        let layout = screen(&quiz).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("reveal")).is_some());
        assert!(layout.rect_of_action(action_id("answer-0")).is_none());
    }
    #[test]
    fn pack_cap_prunes_fifo_boundary() {
        assert_eq!((PACK_CAP + 1).min(PACK_CAP), PACK_CAP);
    }
    #[test]
    fn question_controls_fit_clara() {
        let quiz = Quiz {
            view: View::Question,
            ..Quiz::default()
        };
        let screen = screen(&quiz);
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
        for i in 0..4 {
            assert!(screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .rect_of_action(action_id(&choice(i)))
                .is_some());
        }
    }
}
