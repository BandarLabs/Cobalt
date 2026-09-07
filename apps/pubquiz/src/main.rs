//! Pub Quiz keeps its question packs and play state on the reader.

use kobo_json::Value;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
    Task, TaskId, TaskOutcome,
};
use std::fmt::Write;
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
    Choices,
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

fn question(category: &str, text: &str, answers: [&str; 4], correct: usize) -> Question {
    Question {
        category: category.into(),
        text: text.into(),
        answers: answers.map(Into::into),
        correct,
    }
}

fn bundled_questions() -> Vec<Question> {
    [
        question(
            "Science",
            "Which planet has the shortest year?",
            ["Mercury", "Mars", "Venus", "Earth"],
            0,
        ),
        question(
            "General knowledge",
            "What is the capital of Finland?",
            ["Oslo", "Helsinki", "Tallinn", "Stockholm"],
            1,
        ),
        question(
            "History",
            "Which ship carried Charles Darwin on his voyage?",
            ["Beagle", "Endeavour", "Victory", "Resolution"],
            0,
        ),
        question(
            "Arts",
            "Who painted The Persistence of Memory?",
            ["Miró", "Dalí", "Picasso", "Kahlo"],
            1,
        ),
        question(
            "Geography",
            "Which river runs through Budapest?",
            ["Rhine", "Danube", "Seine", "Tagus"],
            1,
        ),
        question(
            "Science",
            "What is the chemical symbol for gold?",
            ["Ag", "Gd", "Au", "Go"],
            2,
        ),
        question(
            "Literature",
            "Who wrote Frankenstein?",
            [
                "Mary Shelley",
                "George Eliot",
                "Jane Austen",
                "Emily Brontë",
            ],
            0,
        ),
        question(
            "Music",
            "How many strings does a standard violin have?",
            ["Three", "Four", "Five", "Six"],
            1,
        ),
        question(
            "Nature",
            "Which animal is the largest living bird?",
            ["Emu", "Albatross", "Ostrich", "Condor"],
            2,
        ),
        question(
            "Sport",
            "How many players start on a football team?",
            ["Nine", "Ten", "Eleven", "Twelve"],
            2,
        ),
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
    note: Option<String>,
    rounds: u16,
    questions: Vec<Question>,
    round_questions: Vec<Question>,
    page: usize,
    sync_task: Option<TaskId>,
    pack_synced: bool,
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
            note: None,
            rounds: 0,
            questions: bundled_questions(),
            round_questions: bundled_questions(),
            page: 0,
            sync_task: None,
            pack_synced: false,
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
            format!("{}|{}", self.packs, self.rounds).into_bytes(),
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
        self.page = 0;
        let offset = usize::from(self.rounds) * 10 % self.questions.len();
        self.round_questions = self
            .questions
            .iter()
            .cycle()
            .skip(offset)
            .take(10)
            .cloned()
            .collect();
    }
    fn sync(&mut self, context: &mut Context) {
        if self.sync_task.is_some() {
            return;
        }
        self.note = None;
        self.sync_task = context.spawn_retrying(Task::Fetch {
            url: API.into(),
            offset: 0,
            max_bytes: 128 * 1024,
            credential: None,
            headers: Vec::new(),
        });
        if self.sync_task.is_none() {
            self.note = Some("Trivia packs are already updating.".into());
        }
    }
    fn page_count(&self, context: &Context) -> usize {
        if self.view == View::Choices {
            let question = &self.round_questions[self.question % self.round_questions.len()];
            let titles = question
                .answers
                .iter()
                .enumerate()
                .map(|(index, answer)| format!("{} · {answer}", index + 1))
                .collect::<Vec<_>>();
            let rows = titles
                .iter()
                .map(|title| (title.as_str(), ""))
                .collect::<Vec<_>>();
            context.paginate_rows(&rows, true).len()
        } else {
            context.paginate(&question_text(self), true).len()
        }
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(screen_with(self, context));
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
#[cfg(test)]
fn screen(quiz: &Quiz) -> Screen {
    screen_with(quiz, &Context::default())
}

fn question_text(quiz: &Quiz) -> String {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    let mut text = format!(
        "{} · question {} of 10\n\n{}",
        question.category,
        quiz.question + 1,
        question.text
    );
    for (index, answer) in question.answers.iter().enumerate() {
        write!(text, "\n\n{} · {answer}", index + 1).expect("writing to a String");
    }
    text
}

fn question_title(quiz: &Quiz) -> String {
    if quiz.party {
        format!("{} answers", quiz.player_name())
    } else {
        "Solo round".into()
    }
}

fn answer_rows(question: &Question) -> impl Iterator<Item = (String, String, &str, u16)> {
    question.answers.iter().enumerate().map(|(index, answer)| {
        (
            choice(index),
            format!("{} · {answer}", index + 1),
            "",
            u16::try_from(index + 1).expect("four answers"),
        )
    })
}

fn question_screen(quiz: &Quiz, context: &Context) -> Screen {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    let compact = ScreenBuilder::new("pubquiz-question")
        .top_bar(question_title(quiz))
        .secondary(format!(
            "{} · question {} of 10",
            question.category,
            quiz.question + 1
        ))
        .text(&question.text)
        .rows(answer_rows(question))
        .build();
    if compact
        .diagnostics(&context.metrics(), &kobo_sdk::Chrome::default())
        .issues
        .is_empty()
    {
        return compact;
    }
    // Long questions stay complete, including answers that share a prefix.
    // The same measured prose pagination as the reader keeps every word reachable.
    let pages = context.paginate(&question_text(quiz), true);
    let page = quiz.page.min(pages.len().saturating_sub(1));
    let mut builder = ScreenBuilder::new("pubquiz-question").top_bar(question_title(quiz));
    for paragraph in &pages[page] {
        builder = builder.text(paragraph);
    }
    builder
        .page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages.len()).unwrap_or(u16::MAX),
        )
        .action_bar([
            ("previous-page", "Previous"),
            ("next-page", "Next"),
            ("choose", "Answer"),
        ])
        .build()
}

fn choices_screen(quiz: &Quiz, context: &Context) -> Screen {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    let titles = question
        .answers
        .iter()
        .enumerate()
        .map(|(index, answer)| format!("{} · {answer}", index + 1))
        .collect::<Vec<_>>();
    let rows = titles
        .iter()
        .map(|title| (title.as_str(), ""))
        .collect::<Vec<_>>();
    let pages = context.paginate_rows(&rows, true);
    let page = quiz.page.min(pages.len().saturating_sub(1));
    ScreenBuilder::new("pubquiz-choices")
        .top_bar("Choose an answer")
        .rows(pages[page].iter().map(|&index| {
            (
                choice(index),
                titles[index].as_str(),
                "",
                u16::try_from(index + 1).expect("four answers"),
            )
        }))
        .page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages.len()).unwrap_or(u16::MAX),
        )
        .action_bar([
            ("previous-page", "Previous"),
            ("next-page", "Next"),
            ("question", "Question"),
        ])
        .build()
}

#[allow(clippy::too_many_lines)]
fn screen_with(quiz: &Quiz, context: &Context) -> Screen {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    match quiz.view {
        View::Home => {
            let mut b = ScreenBuilder::new("pubquiz-home")
                .top_bar("Pub Quiz")
                .heading("Question packs")
                .secondary(format!(
                    "{} questions ready · {} rounds completed",
                    quiz.questions.len(),
                    quiz.rounds
                ));
            if let Some(note) = &quiz.note {
                b = b.banner(BannerLevel::Info, note);
            }
            b.primary_button("party", "Start pass-around")
                .buttons([
                    ("solo", "Solo round"),
                    (
                        "sync",
                        if quiz.sync_task.is_some() {
                            "Working…"
                        } else {
                            "Sync packs"
                        },
                    ),
                ])
                .buttons([("how-to-play", "How to play"), ("about", "About")])
                .build()
        }
        View::Question => question_screen(quiz, context),
        View::Choices => choices_screen(quiz, context),
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
            .rows((0..if quiz.party { 4 } else { 1 }).map(|i| {
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
                        self.rounds = p.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
                    }
                }
            } else if key == PACK && !self.pack_synced {
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
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.sync_task != Some(task) {
            return;
        }
        self.sync_task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                if let Some(questions) = parse_pack(&bytes) {
                    self.questions = questions;
                    self.pack_synced = true;
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
        if action == action_id("choose") && self.view == View::Question {
            self.view = View::Choices;
            self.page = 0;
        } else if action == action_id("question") && self.view == View::Choices {
            self.view = View::Question;
            self.page = 0;
        } else if matches!(self.view, View::Question | View::Choices)
            && action == action_id("next-page")
        {
            self.page = self
                .page
                .saturating_add(1)
                .min(self.page_count(context).saturating_sub(1));
        } else if matches!(self.view, View::Question | View::Choices)
            && action == action_id("previous-page")
        {
            self.page = self.page.saturating_sub(1);
        } else if action == action_id("party") && self.view == View::Home {
            self.begin(true);
        } else if action == action_id("solo") && self.view == View::Home {
            self.begin(false);
        } else if action == action_id("sync") && self.view == View::Home {
            self.sync(context);
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("how-to-play") {
            self.view = View::HowTo;
        } else if action == ActionId::BACK || action == action_id("home") {
            self.view = View::Home;
        } else if let Some(answer) = (0..4).find(|i| {
            matches!(self.view, View::Question | View::Choices) && action == action_id(&choice(*i))
        }) {
            self.answer = Some(answer);
            self.view = if self.party { View::Pass } else { View::Reveal };
            if answer == self.round_questions[self.question % self.round_questions.len()].correct {
                self.scores[self.player] += 1;
            }
        } else if action == action_id("reveal") && self.view == View::Pass {
            self.view = View::Reveal;
        } else if action == action_id("continue") && self.view == View::Reveal {
            self.question += 1;
            self.player = if self.party { (self.player + 1) % 4 } else { 0 };
            self.page = 0;
            self.answer = None;
            if self.question >= 10 {
                self.view = View::Podium;
                self.rounds = self.rounds.saturating_add(1);
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

#[cfg(test)]
mod regression_tests {
    use super::*;
    use kobo_sdk::{AppRunner, Chrome, DisplayMetrics};
    use kobo_ui::TextScale;

    fn pack() -> Vec<u8> {
        let item = r#"{"category":"New pack","question":"Changed question?","correct_answer":"Right","incorrect_answers":["Wrong 1","Wrong 2","Wrong 3"]}"#;
        format!(
            r#"{{"response_code":0,"results":[{}]}}"#,
            [item; 10].join(",")
        )
        .into_bytes()
    }

    #[test]
    fn solo_round_credits_only_the_solo_player_once() {
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("solo"));
        for _ in 0..10 {
            let quiz = runner.app();
            let correct = quiz.round_questions[quiz.question].correct;
            runner.action(action_id(&choice(correct)));
            runner.action(action_id(&choice(correct))); // A stale double tap cannot score twice.
            runner.action(action_id("continue"));
        }
        assert_eq!(runner.app().scores, [10, 0, 0, 0]);
        assert_eq!(runner.app().rounds, 1);
        runner.action(action_id("continue"));
        assert_eq!(runner.app().rounds, 1);
        assert!(!format!("{:?}", screen(runner.app())).contains("Bert"));
    }

    #[test]
    fn late_cache_and_sync_do_not_replace_an_active_round() {
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("sync started");
        runner.action(action_id("solo"));
        let round = runner.app().round_questions.clone();
        runner.store_result(StoreResult::Loaded {
            key: PACK.into(),
            value: Some(pack()),
        });
        assert_eq!(runner.app().round_questions, round);
        runner.task_outcome(task, TaskOutcome::Completed(pack()));
        assert_eq!(runner.app().round_questions, round);
        runner.action(action_id("home"));
        runner.action(action_id("solo"));
        assert_eq!(runner.app().round_questions[0].category, "New pack");
    }

    #[test]
    fn long_questions_keep_every_answer_reachable_at_large_text_sizes() {
        let mut quiz = Quiz::default();
        quiz.begin(false);
        quiz.round_questions[0].text =
            "A long question about the people involved in a historical event. "
                .repeat(4)
                .chars()
                .take(240)
                .collect();
        quiz.round_questions[0].answers = std::array::from_fn(|index| {
            format!(
                "{} ending{index}",
                "A long answer with shared words ".repeat(3)
            )
            .chars()
            .rev()
            .take(80)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
        });
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (758, 1024, 212), (1448, 1072, 300)]
        {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let context = AppRunner::with_metrics(Quiz::default(), metrics).context();
                let pages = context.paginate(&question_text(&quiz), true);
                for page in 0..pages.len() {
                    quiz.page = page;
                    let screen = question_screen(&quiz, &context);
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                }
                let mut seen = [false; 4];
                for page in 0..4 {
                    quiz.page = page;
                    let screen = choices_screen(&quiz, &context);
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    let layout = screen.layout_with(&metrics, &Chrome::default());
                    for (index, found) in seen.iter_mut().enumerate() {
                        *found |= layout.rect_of_action(action_id(&choice(index))).is_some();
                    }
                }
                assert!(seen.into_iter().all(|found| found));
            }
        }
    }
}

#[cfg(test)]
mod help_layout_tests {
    use super::*;
    #[test]
    fn help_fits_supported_text_scales_and_geometries() {
        let quiz = Quiz {
            view: View::HowTo,
            ..Quiz::default()
        };
        let screens = [screen(&quiz)];
        for screen in screens {
            for (width, height, pixels_per_inch) in
                [(1072, 1448, 300), (758, 1024, 212), (1448, 1072, 300)]
            {
                for text_scale in kobo_ui::TextScale::STEPS {
                    let metrics = kobo_sdk::DisplayMetrics {
                        width,
                        height,
                        pixels_per_inch,
                        text_scale,
                    };
                    let chrome = kobo_ui::Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    assert!(screen
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id("home"))
                        .is_some());
                }
            }
        }
    }
}
