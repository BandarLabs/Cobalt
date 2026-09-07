//! Pub Quiz keeps its question packs and play state on the reader.

use kobo_json::Value;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
    Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const STATE: &str = "pubquiz-state";
const PACK: &str = "pubquiz-pack-v1";
const LICENSE: &str = "pubquiz-content-license";
const LICENSE_TEXT: &str = "Questions: Open Trivia DB (opentdb.com), CC-BY-SA 4.0. Cached question content remains under CC-BY-SA 4.0.";
const API: &str = "https://opentdb.com/api.php?amount=50&type=multiple";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Question,
    Pass,
    Reveal,
    Podium,
    HowTo,
    About,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Question {
    category: String,
    text: String,
    answers: [String; 4],
    correct: usize,
}
fn bundled_questions() -> Vec<Question> {
    [
        Question {
            category: "Science".into(),
            text: "Which planet has the shortest year?".into(),
            answers: [
                "Mercury".into(),
                "Mars".into(),
                "Venus".into(),
                "Earth".into(),
            ],
            correct: 0,
        },
        Question {
            category: "General knowledge".into(),
            text: "What is the capital of Finland?".into(),
            answers: [
                "Oslo".into(),
                "Helsinki".into(),
                "Tallinn".into(),
                "Stockholm".into(),
            ],
            correct: 1,
        },
        Question {
            category: "History".into(),
            text: "Which ship carried Charles Darwin on his voyage?".into(),
            answers: [
                "Beagle".into(),
                "Endeavour".into(),
                "Victory".into(),
                "Resolution".into(),
            ],
            correct: 0,
        },
        Question {
            category: "Arts".into(),
            text: "Who painted The Persistence of Memory?".into(),
            answers: [
                "Miró".into(),
                "Dalí".into(),
                "Picasso".into(),
                "Kahlo".into(),
            ],
            correct: 1,
        },
        Question {
            category: "Geography".into(),
            text: "Which river runs through Budapest?".into(),
            answers: [
                "Rhine".into(),
                "Danube".into(),
                "Seine".into(),
                "Tagus".into(),
            ],
            correct: 1,
        },
        Question {
            category: "Science".into(),
            text: "What is the chemical symbol for gold?".into(),
            answers: ["Ag".into(), "Gd".into(), "Au".into(), "Go".into()],
            correct: 2,
        },
        Question {
            category: "Literature".into(),
            text: "Who wrote Frankenstein?".into(),
            answers: [
                "Mary Shelley".into(),
                "George Eliot".into(),
                "Jane Austen".into(),
                "Emily Brontë".into(),
            ],
            correct: 0,
        },
        Question {
            category: "Music".into(),
            text: "How many strings does a standard violin have?".into(),
            answers: ["Three".into(), "Four".into(), "Five".into(), "Six".into()],
            correct: 1,
        },
        Question {
            category: "Nature".into(),
            text: "Which animal is the largest living bird?".into(),
            answers: [
                "Emu".into(),
                "Albatross".into(),
                "Ostrich".into(),
                "Condor".into(),
            ],
            correct: 2,
        },
        Question {
            category: "Sport".into(),
            text: "How many players start on a football team?".into(),
            answers: [
                "Nine".into(),
                "Ten".into(),
                "Eleven".into(),
                "Twelve".into(),
            ],
            correct: 2,
        },
    ]
    .into()
}
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
    questions: Vec<Question>,
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
            questions: bundled_questions(),
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
            self.note = Some("Trivia packs are already updating.".into());
        }
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(screen(self));
    }
}
fn choice(index: usize) -> String {
    format!("answer-{index}")
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" | "#039" | "#39" => Some('\''),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "nbsp" => Some(' '),
        "eacute" => Some('é'),
        "ouml" => Some('ö'),
        "uuml" => Some('ü'),
        _ => entity
            .strip_prefix("#x")
            .or_else(|| entity.strip_prefix("#X"))
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .and_then(char::from_u32)
            .or_else(|| {
                entity
                    .strip_prefix('#')
                    .and_then(|digits| digits.parse::<u32>().ok())
                    .and_then(char::from_u32)
            }),
    }
}

fn clean_text(input: &str, limit: usize) -> String {
    let mut output = String::new();
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
            output.push('&');
            rest = &rest[1..];
            continue;
        };
        if let Some(decoded) = decode_entity(&rest[1..end]) {
            output.push(decoded);
            rest = &rest[end + 1..];
        } else {
            output.push('&');
            rest = &rest[1..];
        }
    }
    output.push_str(rest);
    let normalized = output.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(limit).collect()
}

fn parse_pack(bytes: &[u8]) -> Option<Vec<Question>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value = kobo_json::parse(text).ok()?;
    if value.get("response_code").and_then(Value::as_i64) != Some(0) {
        return None;
    }
    let results = value.get("results")?.as_array()?;
    let questions = results
        .iter()
        .filter_map(|item| {
            let category = clean_text(item.get("category")?.as_str()?, 48);
            let text = clean_text(item.get("question")?.as_str()?, 240);
            let correct = clean_text(item.get("correct_answer")?.as_str()?, 80);
            let wrong = item.get("incorrect_answers")?.as_array()?;
            if category.is_empty() || text.is_empty() || correct.is_empty() || wrong.len() != 3 {
                return None;
            }
            let mut answers = wrong
                .iter()
                .map(|answer| clean_text(answer.as_str().unwrap_or_default(), 80))
                .collect::<Vec<_>>();
            if answers.iter().any(String::is_empty) {
                return None;
            }
            let slot = text.bytes().fold(0_usize, |hash, byte| {
                hash.wrapping_mul(33).wrapping_add(usize::from(byte))
            }) % 4;
            answers.insert(slot, correct);
            let answers: [String; 4] = answers.try_into().ok()?;
            Some(Question {
                category,
                text,
                answers,
                correct: slot,
            })
        })
        .take(50)
        .collect::<Vec<_>>();
    (questions.len() >= 10).then_some(questions)
}
#[allow(clippy::too_many_lines)]
fn screen(quiz: &Quiz) -> Screen {
    let question = &quiz.questions[quiz.question % quiz.questions.len()];
    match quiz.view {
        View::Home => {
            let mut b = ScreenBuilder::new("pubquiz-home")
                .top_bar("Pub Quiz")
                .heading("Question packs")
                .secondary(format!(
                    "{} questions ready · {} day streak",
                    quiz.questions.len(),
                    quiz.streak
                ));
            if let Some(note) = &quiz.note {
                b = b.banner(BannerLevel::Info, note);
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
                .buttons([("how-to-play", "How to play"), ("about", "About")])
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
            .heading(question.text.clone())
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
        View::HowTo => ScreenBuilder::new("pubquiz-help")
            .top_bar("How to play")
            .owns_back(true)
            .heading("Ten questions, one Kobo")
            .text("Solo: choose an answer and see the result right away.")
            .text("Pass-around: answer, pass the Kobo, then reveal the result.")
            .text("Players take turns. The highest score after ten questions wins.")
            .bottom_action("home", "Play")
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
        context.store().load(PACK);
        context
            .store()
            .save(LICENSE, LICENSE_TEXT.as_bytes().to_vec());
        self.show(context);
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == STATE {
                if let Some(bytes) = value {
                    if let Ok(s) = String::from_utf8(bytes) {
                        let p: Vec<_> = s.split('|').collect();
                        self.packs = p.first().and_then(|x| x.parse().ok()).unwrap_or(0);
                        self.streak = p.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                    }
                }
                self.loaded = true;
            } else if key == PACK {
                if let Some(bytes) = value {
                    if let Some(questions) = parse_pack(&bytes) {
                        self.questions = questions;
                        self.packs = 1;
                    } else {
                        context.store().forget(PACK);
                        self.note =
                            Some("Saved questions were damaged; the built-in set is ready.".into());
                    }
                }
            }
            self.show(context);
        }
    }
    fn on_task(&mut self, context: &mut Context, _: TaskId, outcome: TaskOutcome) {
        self.syncing = false;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                if let Some(questions) = parse_pack(&bytes) {
                    self.questions = questions;
                    self.packs = 1;
                    context.store().save(PACK, bytes);
                    self.note = Some(format!(
                        "{} fresh questions saved for offline play.",
                        self.questions.len()
                    ));
                    self.save(context);
                } else {
                    self.note = Some("Open Trivia DB returned no usable question set.".into());
                }
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
        } else if action == action_id("how-to-play") {
            self.view = View::HowTo;
        } else if action == ActionId::BACK {
            self.view = View::Home;
        } else if action == action_id("home") {
            self.view = View::Home;
        } else if let Some(answer) = (0..4).find(|i| action == action_id(&choice(*i))) {
            self.answer = Some(answer);
            self.view = if self.party { View::Pass } else { View::Reveal };
            if answer == self.questions[self.question % self.questions.len()].correct {
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
    fn entities_decode_before_render() {
        assert_eq!(
            clean_text("Rock &amp; Roll &#039;A&#039;", 80),
            "Rock & Roll 'A'"
        );
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

    #[test]
    fn a_round_has_ten_distinct_questions_and_short_help() {
        assert_eq!(bundled_questions().len(), 10);
        let quiz = Quiz {
            view: View::HowTo,
            ..Quiz::default()
        };
        assert!(screen(&quiz)
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }

    #[test]
    fn open_trivia_pack_is_parsed_bounded_and_mixed() {
        let mut items = Vec::new();
        for index in 0..10 {
            items.push(format!(
                r#"{{"category":"Science &amp; Nature","question":"Question {index}?","correct_answer":"Right","incorrect_answers":["Wrong 1","Wrong 2","Wrong 3"]}}"#
            ));
        }
        let body = format!(r#"{{"response_code":0,"results":[{}]}}"#, items.join(","));
        let questions = parse_pack(body.as_bytes()).expect("valid pack");
        assert_eq!(questions.len(), 10);
        assert_eq!(questions[0].category, "Science & Nature");
        assert!(questions.iter().all(|question| question.answers.len() == 4));
        assert!(parse_pack(br#"{"response_code":1,"results":[]}"#).is_none());
    }

    #[test]
    fn action_graph_reaches_party_views() {
        use kobo_sdk::AppRunner;
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        assert_eq!(runner.app().view, View::Home);
        runner.action(action_id("about"));
        assert_eq!(runner.app().view, View::About);
        runner.action(action_id("home"));
        runner.action(action_id("party"));
        assert_eq!(runner.app().view, View::Question);
        runner.action(action_id(&choice(0)));
        assert_eq!(runner.app().view, View::Pass);
        runner.action(action_id("reveal"));
        assert_eq!(runner.app().view, View::Reveal);
        runner.app_mut().question = 9;
        runner.action(action_id("continue"));
        assert_eq!(runner.app().view, View::Podium);
        runner.action(action_id("home"));
        assert_eq!(runner.app().view, View::Home);
    }
}
