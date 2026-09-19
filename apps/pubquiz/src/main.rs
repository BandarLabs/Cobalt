//! Pub Quiz keeps its question packs and play state on the reader.

use kobo_json::Value;
use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::{
    action_id,
    keyboard::{TextEntry, Typing},
    ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult, Task,
    TaskId, TaskOutcome,
};
use kobo_ui::TextScale;
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
    Setup,
    Players,
    Question,
    Choices,
    Pass,
    Reveal,
    Podium,
    HowTo,
    About,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Difficulty {
    Easy,
    Medium,
    Hard,
}
impl Difficulty {
    fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }
    fn from_pack(value: &str) -> Self {
        match value {
            "easy" => Self::Easy,
            "hard" => Self::Hard,
            _ => Self::Medium,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Question {
    category: String,
    difficulty: Difficulty,
    text: String,
    answers: [String; 4],
    correct: usize,
}

fn question(
    category: &str,
    difficulty: Difficulty,
    text: &str,
    answers: [&str; 4],
    correct: usize,
) -> Question {
    Question {
        category: category.into(),
        difficulty,
        text: text.into(),
        answers: answers.map(Into::into),
        correct,
    }
}

fn bundled_questions() -> Vec<Question> {
    [
        question(
            "Science",
            Difficulty::Easy,
            "Which planet has the shortest year?",
            ["Mercury", "Mars", "Venus", "Earth"],
            0,
        ),
        question(
            "General knowledge",
            Difficulty::Easy,
            "What is the capital of Finland?",
            ["Oslo", "Helsinki", "Tallinn", "Stockholm"],
            1,
        ),
        question(
            "History",
            Difficulty::Hard,
            "Which ship carried Charles Darwin on his voyage?",
            ["Beagle", "Endeavour", "Victory", "Resolution"],
            0,
        ),
        question(
            "Arts",
            Difficulty::Medium,
            "Who painted The Persistence of Memory?",
            ["Miró", "Dalí", "Picasso", "Kahlo"],
            1,
        ),
        question(
            "Geography",
            Difficulty::Easy,
            "Which river runs through Budapest?",
            ["Rhine", "Danube", "Seine", "Tagus"],
            1,
        ),
        question(
            "Science",
            Difficulty::Easy,
            "What is the chemical symbol for gold?",
            ["Ag", "Gd", "Au", "Go"],
            2,
        ),
        question(
            "Literature",
            Difficulty::Medium,
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
            Difficulty::Easy,
            "How many strings does a standard violin have?",
            ["Three", "Four", "Five", "Six"],
            1,
        ),
        question(
            "Nature",
            Difficulty::Medium,
            "Which animal is the largest living bird?",
            ["Emu", "Albatross", "Ostrich", "Condor"],
            2,
        ),
        question(
            "Sport",
            Difficulty::Easy,
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
    players: usize,
    names: [String; 4],
    renaming: usize,
    entry: TextEntry,
    question: usize,
    answer: Option<usize>,
    scores: [u8; 4],
    packs: u8,
    note: Option<String>,
    rounds: u16,
    questions: Vec<Question>,
    round_questions: Vec<Question>,
    pack_origin: Option<String>,
    pack_updated_min: Option<i64>,
    setup_party: bool,
    setup_category: Option<String>,
    setup_difficulty: Option<Difficulty>,
    setup_page: usize,
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
            players: 4,
            names: ["Ada", "Bert", "Cleo", "Dev"].map(String::from),
            renaming: 0,
            entry: TextEntry::new(),
            question: 0,
            answer: None,
            scores: [0; 4],
            packs: 0,
            note: None,
            rounds: 0,
            questions: bundled_questions(),
            round_questions: bundled_questions(),
            pack_origin: None,
            pack_updated_min: None,
            setup_party: true,
            setup_category: None,
            setup_difficulty: None,
            setup_page: 0,
            page: 0,
            sync_task: None,
            pack_synced: false,
        }
    }
}
impl Quiz {
    fn player_name(&self) -> &str {
        if self.party {
            &self.names[self.player]
        } else {
            "You"
        }
    }
    fn name_for(&self, index: usize) -> &str {
        if self.party {
            &self.names[index]
        } else {
            "You"
        }
    }
    fn state_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.packs,
            self.rounds,
            self.players,
            self.names.join(","),
            self.pack_origin.as_deref().unwrap_or(""),
            self.pack_updated_min
                .map_or_else(String::new, |minutes| minutes.to_string())
        )
    }
    fn source_label(&self) -> &str {
        self.pack_origin.as_deref().unwrap_or("Bundled")
    }
    fn updated_label(&self) -> String {
        match (self.pack_origin.is_some(), self.pack_updated_min) {
            (false, _) => "Not synced yet.".to_owned(),
            (true, None) => "Synced earlier.".to_owned(),
            (true, Some(minutes)) => match now_minutes() {
                Some(now) if now >= minutes => {
                    format!("Synced {}.", age_label((now - minutes) * 60))
                }
                _ => "Synced earlier.".to_owned(),
            },
        }
    }
    fn save(&self, context: &mut Context) {
        context.store().save(STATE, self.state_line().into_bytes());
    }
    fn setup_action(&mut self, action: ActionId, context: &Context) -> bool {
        if action == action_id("diff-cycle") {
            self.setup_difficulty = match self.setup_difficulty {
                None => Some(Difficulty::Easy),
                Some(Difficulty::Easy) => Some(Difficulty::Medium),
                Some(Difficulty::Medium) => Some(Difficulty::Hard),
                Some(Difficulty::Hard) => None,
            };
        } else if action == action_id("cat-any") {
            self.setup_category = None;
        } else if action == action_id("next-page") {
            let rows_per_page = setup_rows_per_page(context);
            let pages = setup_categories(self).len().max(1).div_ceil(rows_per_page);
            self.setup_page = (self.setup_page + 1).min(pages - 1);
        } else if action == action_id("previous-page") {
            self.setup_page = self.setup_page.saturating_sub(1);
        } else if action == action_id("continue-setup") {
            if self.setup_party {
                self.view = View::Players;
            } else {
                self.begin(false);
            }
        } else if action != ActionId::BACK && action != action_id("home") {
            let categories = setup_categories(self);
            let rows_per_page = setup_rows_per_page(context);
            let pages = categories.len().max(1).div_ceil(rows_per_page);
            let page = self.setup_page.min(pages - 1);
            let offset = usize::from(page == 0);
            let Some(index) = (0..rows_per_page.saturating_sub(offset))
                .find(|i| action == action_id(&format!("cat-{i}")))
            else {
                return false;
            };
            if let Some(name) = categories.get(page * SETUP_ROWS + index) {
                self.setup_category = Some(name.clone());
            }
        } else {
            return false;
        }
        true
    }

    fn begin(&mut self, party: bool) {
        let pool: Vec<Question> = self
            .questions
            .iter()
            .filter(|question| {
                self.setup_category
                    .as_ref()
                    .is_none_or(|category| &question.category == category)
                    && self
                        .setup_difficulty
                        .is_none_or(|difficulty| question.difficulty == difficulty)
            })
            .cloned()
            .collect();
        if pool.is_empty() {
            self.note =
                Some("No questions match that mix yet. Sync packs or widen the choice.".into());
            self.view = View::Home;
            return;
        }
        self.party = party;
        self.view = View::Question;
        self.question = 0;
        self.player = 0;
        self.answer = None;
        self.scores = [0; 4];
        self.note = None;
        self.page = 0;
        let offset = usize::from(self.rounds) * 10 % pool.len();
        self.round_questions = pool.iter().cycle().skip(offset).take(10).cloned().collect();
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
            let difficulty = item
                .get("difficulty")
                .and_then(Value::as_str)
                .map_or(Difficulty::Medium, Difficulty::from_pack);
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
                difficulty,
                text,
                answers,
                correct: slot,
            })
        })
        .take(50)
        .collect::<Vec<_>>();
    (questions.len() >= 10).then_some(questions)
}
fn reader_clock() -> Box<dyn Clock> {
    let minutes = std::env::var("KOBO_UTC_OFFSET_MINUTES")
        .ok()
        .and_then(|value| value.parse::<i16>().ok())
        .unwrap_or(0);
    SystemClock::new(minutes).map_or_else(
        |_| {
            Box::new(
                ManualClock::new(Snapshot {
                    unix_millis: 0,
                    monotonic_millis: 0,
                    utc_offset_minutes: 0,
                })
                .expect("a valid fixed clock"),
            ) as Box<dyn Clock>
        },
        |clock| Box::new(clock) as Box<dyn Clock>,
    )
}

fn now_minutes() -> Option<i64> {
    reader_clock()
        .now()
        .ok()
        .map(|snapshot| i64::try_from(snapshot.unix_millis / 60_000).unwrap_or(i64::MAX))
}

fn age_label(seconds: i64) -> String {
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{} min ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{} hr ago", seconds / 3600)
    } else {
        format!("{} days ago", seconds / 86400)
    }
}

#[cfg(test)]
fn screen(quiz: &Quiz) -> Screen {
    screen_with(quiz, &Context::default())
}

fn question_text(quiz: &Quiz) -> String {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    let mut text = format!(
        "{} · question {} of {}\n\n{}",
        question.category,
        quiz.question + 1,
        quiz.round_questions.len(),
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
            answer.clone(),
            "",
            u16::try_from(index + 1).expect("four answers"),
        )
    })
}

fn about_screen(quiz: &Quiz) -> Screen {
    ScreenBuilder::new("pubquiz-about")
        .top_bar("Pub Quiz")
        .heading("About")
        .text("Question packs use Open Trivia DB content, licensed CC-BY-SA 4.0.")
        .text("opentdb.com · cached packs are redistributed under the same license.")
        .rows([
            (
                "about-source".to_owned(),
                "Source".to_owned(),
                quiz.source_label().to_owned(),
                Glyph::Download,
            ),
            (
                "about-updated".to_owned(),
                "Updated".to_owned(),
                quiz.updated_label(),
                Glyph::Clock,
            ),
        ])
        .button("home", "Back to packs")
        .build()
}

fn question_screen(quiz: &Quiz, context: &Context) -> Screen {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    let compact = ScreenBuilder::new("pubquiz-question")
        .top_bar(question_title(quiz))
        .secondary(format!(
            "{} · question {} of {}",
            question.category,
            quiz.question + 1,
            quiz.round_questions.len()
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
    let titles = question.answers.to_vec();
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
/// A typed player name: trimmed, free of the state file's separators, and
/// short enough to fit a podium row.
fn clean_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect::<String>()
        .trim()
        .chars()
        .take(12)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

const SETUP_ROWS: usize = 3;

fn setup_categories(quiz: &Quiz) -> Vec<String> {
    let mut categories: Vec<String> = quiz
        .questions
        .iter()
        .map(|question| question.category.clone())
        .collect();
    categories.sort();
    categories.dedup();
    categories
}

fn setup_screen_with(quiz: &Quiz, categories: &[String], rows_per_page: usize) -> Screen {
    let pages = categories.len().max(1).div_ceil(rows_per_page);
    let page = quiz.setup_page.min(pages - 1);
    let window =
        &categories[page * rows_per_page..categories.len().min((page + 1) * rows_per_page)];
    let difficulty = quiz.setup_difficulty.map_or("Any", Difficulty::label);
    let mut rows: Vec<(String, String, String, Glyph)> = vec![(
        "diff-cycle".to_owned(),
        "Difficulty".to_owned(),
        difficulty.to_owned(),
        Glyph::Grid,
    )];
    if page == 0 {
        rows.push((
            "cat-any".to_owned(),
            "Any category".to_owned(),
            if quiz.setup_category.is_none() {
                "Chosen".to_owned()
            } else {
                "Choose".to_owned()
            },
            Glyph::Grid,
        ));
    }
    for (index, name) in window.iter().enumerate() {
        let title = if name.chars().count() > 18 {
            format!("{}\u{2026}", name.chars().take(17).collect::<String>())
        } else {
            name.clone()
        };
        rows.push((
            format!("cat-{index}"),
            title,
            if quiz.setup_category.as_ref() == Some(name) {
                "Chosen".to_owned()
            } else {
                "Choose".to_owned()
            },
            Glyph::Grid,
        ));
    }
    ScreenBuilder::new("pubquiz-setup")
        .top_bar("Pub Quiz")
        .heading("Round setup")
        .rows(rows)
        .page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages).unwrap_or(u16::MAX),
        )
        .action_bar([("previous-page", "Previous"), ("next-page", "Next")])
        .primary_button("continue-setup", "Continue")
        .build()
}

/// Bigger text leaves less vertical room: extra-large fits one category a
/// page beside the difficulty row, large fits two, anything smaller three.
fn setup_rows_per_page(context: &Context) -> usize {
    match context.metrics().text_scale {
        TextScale::Larger | TextScale::ExtraLarge => 1,
        TextScale::Large | TextScale::Medium => 2,
        _ => SETUP_ROWS,
    }
}

fn setup_screen(quiz: &Quiz, context: &Context) -> Screen {
    setup_screen_with(quiz, &setup_categories(quiz), setup_rows_per_page(context))
}

fn players_screen(quiz: &Quiz) -> Screen {
    ScreenBuilder::new("pubquiz-players")
        .top_bar("Pub Quiz")
        .heading("Pass-around players")
        .secondary(format!("{} players take turns.", quiz.players))
        .buttons([("count-2", "2"), ("count-3", "3"), ("count-4", "4")])
        .rows((0..quiz.players).map(|i| {
            (
                format!("rename-{i}"),
                quiz.names[i].as_str(),
                "Rename",
                Glyph::Person,
            )
        }))
        .primary_button("start", "Start round")
        .build()
}

fn reveal_screen(quiz: &Quiz, question: &Question) -> Screen {
    let right = quiz.answer == Some(question.correct);
    let mut builder = ScreenBuilder::new("pubquiz-reveal")
        .top_bar("Round result")
        .heading(if right { "Correct" } else { "Not this time" })
        .secondary(format!(
            "{} · {}",
            question.category, question.answers[question.correct]
        ));
    if !right {
        if let Some(chosen) = quiz.answer {
            let who = if quiz.party {
                quiz.player_name()
            } else {
                "You"
            };
            builder = builder.text(format!("{who} chose {}", question.answers[chosen]));
        }
    }
    let players = if quiz.party { quiz.players } else { 1 };
    // Nobody has scored yet: four rows of zeroes say nothing, so the
    // scoreboard stays away until the first point exists.
    if quiz.scores[..players].iter().all(|&score| score == 0) {
        if quiz.party {
            builder = builder.text("No points yet.");
        }
    } else {
        builder = builder
            .facts((0..players).map(|i| (quiz.name_for(i), format!("{} points", quiz.scores[i]))));
    }
    builder
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

fn screen_with(quiz: &Quiz, context: &Context) -> Screen {
    if quiz.entry.is_open() {
        return ScreenBuilder::new("pubquiz-rename")
            .top_bar("Pub Quiz")
            .secondary("Letters and digits, twelve characters or fewer.")
            .text_entry(&quiz.entry, "Player name", "Done")
            .build();
    }
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    match quiz.view {
        View::Setup => setup_screen(quiz, context),
        View::Players => players_screen(quiz),
        View::Home => {
            let mut b = ScreenBuilder::new("pubquiz-home")
                .top_bar("Pub Quiz")
                .heading("Question packs")
                .secondary(format!(
                    "{} questions · {} · {} rounds completed",
                    quiz.questions.len(),
                    quiz.source_label(),
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
        View::Pass => {
            let next = quiz.name_for((quiz.player + 1) % quiz.players).to_owned();
            ScreenBuilder::new("pubquiz-pass")
                .top_bar("Pass it on")
                .heading("Answer locked")
                .text(format!(
                    "{} answered. Hand the Kobo to {next}, who reveals the result.",
                    quiz.player_name()
                ))
                .primary_button("reveal", "Show result")
                .build()
        }
        View::Reveal => reveal_screen(quiz, question),
        View::Podium => {
            let count = if quiz.party { quiz.players } else { 1 };
            let mut order: Vec<usize> = (0..count).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(quiz.scores[i]));
            ScreenBuilder::new("pubquiz-podium")
                .top_bar("Pub Quiz")
                .heading("Podium")
                .rows(order.into_iter().map(|i| {
                    (
                        format!("player-{i}"),
                        quiz.name_for(i),
                        format!("{} points", quiz.scores[i]),
                        Glyph::Person,
                    )
                }))
                .primary_button("home", "Finish round")
                .build()
        }
        View::HowTo => ScreenBuilder::new("pubquiz-help")
            .top_bar("How to play")
            .owns_back(true)
            .heading("One Kobo, every player")
            .text("Solo: choose an answer and see the result right away.")
            .text("Pass-around: answer, pass the Kobo, then reveal the result.")
            .text("Players take turns. The highest score after the last question wins.")
            .bottom_action("home", "Play")
            .build(),
        View::About => about_screen(quiz),
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
                        if let Some(players) = p.get(2).and_then(|x| x.parse().ok()) {
                            self.players = players;
                        }
                        if let Some(names) = p.get(3) {
                            for (slot, name) in self.names.iter_mut().zip(names.split(',')) {
                                if let Some(name) = clean_name(name) {
                                    *slot = name;
                                }
                            }
                        }
                        if let Some(origin) = p.get(4) {
                            self.pack_origin = (!origin.is_empty()).then(|| (*origin).to_string());
                        }
                        if let Some(minutes) = p.get(5).and_then(|m| m.parse().ok()) {
                            self.pack_updated_min = Some(minutes);
                        }
                    }
                }
            } else if key == PACK && !self.pack_synced {
                if let Some(bytes) = value {
                    if let Some(questions) = parse_pack(&bytes) {
                        self.questions = questions;
                        self.packs = 1;
                        if self.pack_origin.is_none() {
                            self.pack_origin = Some("Open Trivia DB".to_owned());
                        }
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
                    self.pack_origin = Some("Open Trivia DB".to_owned());
                    self.pack_updated_min = now_minutes();
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
        if let Some(event) = self.entry.handle(action) {
            if let Typing::Submitted(name) = event {
                if let Some(name) = clean_name(&name) {
                    self.names[self.renaming] = name;
                    self.save(context);
                }
            }
            self.show(context);
            return;
        }
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
            self.setup_party = true;
            self.setup_page = 0;
            self.view = View::Setup;
        } else if self.view == View::Setup && self.setup_action(action, context) {
        } else if self.view == View::Players && action == action_id("start") {
            self.begin(true);
        } else if self.view == View::Players && action == action_id("count-2") {
            self.players = 2;
            self.save(context);
        } else if self.view == View::Players && action == action_id("count-3") {
            self.players = 3;
            self.save(context);
        } else if self.view == View::Players && action == action_id("count-4") {
            self.players = 4;
            self.save(context);
        } else if self.view == View::Players {
            if let Some(index) =
                (0..self.players).find(|i| action == action_id(&format!("rename-{i}")))
            {
                self.renaming = index;
                self.entry.open();
            }
        } else if action == action_id("solo") && self.view == View::Home {
            self.setup_party = false;
            self.setup_page = 0;
            self.view = View::Setup;
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
            self.player = if self.party {
                (self.player + 1) % self.players
            } else {
                0
            };
            self.page = 0;
            self.answer = None;
            if self.question >= self.round_questions.len() {
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
        assert_eq!(runner.app().view, View::Setup);
        runner.action(action_id("continue-setup"));
        assert_eq!(runner.app().view, View::Players);
        runner.action(action_id("start"));
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

    #[test]
    fn pack_difficulty_is_mapped_when_present() {
        let easy = r#"{"category":"Science","difficulty":"easy","question":"Q?","correct_answer":"Right","incorrect_answers":["W1","W2","W3"]}"#;
        let hard = r#"{"category":"Science","difficulty":"hard","question":"Q?","correct_answer":"Right","incorrect_answers":["W1","W2","W3"]}"#;
        let plain = r#"{"category":"Science","question":"Q?","correct_answer":"Right","incorrect_answers":["W1","W2","W3"]}"#;
        let body = format!(
            r#"{{"response_code":0,"results":[{}]}}"#,
            [easy, hard, plain].repeat(4).join(",")
        );
        let questions = parse_pack(body.as_bytes()).expect("valid pack");
        assert_eq!(questions.len(), 12);
        assert_eq!(questions[0].difficulty, Difficulty::Easy);
        assert_eq!(questions[1].difficulty, Difficulty::Hard);
        assert_eq!(questions[2].difficulty, Difficulty::Medium);
    }

    #[test]
    fn setup_filters_the_round_and_refuses_an_empty_mix() {
        let mut quiz = Quiz {
            setup_category: Some("Science".to_owned()),
            ..Quiz::default()
        };
        quiz.begin(false);
        assert!(!quiz.round_questions.is_empty());
        assert!(quiz
            .round_questions
            .iter()
            .all(|question| question.category == "Science"));

        let mut quiz = Quiz {
            setup_category: Some("No such category".to_owned()),
            ..Quiz::default()
        };
        let kept = quiz.round_questions.clone();
        quiz.begin(false);
        assert_eq!(quiz.round_questions, kept);
        assert_eq!(quiz.view, View::Home);
        assert!(quiz.note.is_some());

        let mut quiz = Quiz {
            setup_difficulty: Some(Difficulty::Easy),
            ..Quiz::default()
        };
        quiz.begin(false);
        assert!(quiz
            .round_questions
            .iter()
            .all(|question| question.difficulty == Difficulty::Easy));
    }

    #[test]
    fn setup_flow_applies_the_chosen_mix() {
        use kobo_sdk::AppRunner;
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("party"));
        assert_eq!(runner.app().view, View::Setup);
        runner.action(action_id("diff-cycle"));
        assert_eq!(runner.app().setup_difficulty, Some(Difficulty::Easy));
        runner.action(action_id("continue-setup"));
        assert_eq!(runner.app().view, View::Players);
        runner.action(action_id("start"));
        assert_eq!(runner.app().view, View::Question);
        assert!(runner
            .app()
            .round_questions
            .iter()
            .all(|question| question.difficulty == Difficulty::Easy));
    }

    #[test]
    fn pack_source_and_freshness_survive_save_and_load() {
        let quiz = Quiz {
            packs: 1,
            rounds: 4,
            players: 2,
            pack_origin: Some("Open Trivia DB".to_owned()),
            pack_updated_min: Some(31_556_000),
            ..Quiz::default()
        };
        assert!(quiz.state_line().ends_with("|Open Trivia DB|31556000"));

        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(b"1|4|2|Sam,Bo,Cleo,Dev|Open Trivia DB|31556000".to_vec()),
        });
        assert_eq!(runner.app().pack_origin.as_deref(), Some("Open Trivia DB"));
        assert_eq!(runner.app().pack_updated_min, Some(31_556_000));
        assert_eq!(runner.app().updated_label(), "Synced earlier.");

        // States saved before freshness tracking still load, as bundled.
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(b"1|5".to_vec()),
        });
        assert_eq!(runner.app().pack_origin, None);
        assert_eq!(runner.app().source_label(), "Bundled");
        assert_eq!(runner.app().updated_label(), "Not synced yet.");
    }

    #[test]
    fn about_names_the_pack_source() {
        let bundled = format!(
            "{:?}",
            screen(&Quiz {
                view: View::About,
                ..Quiz::default()
            })
        );
        assert!(bundled.contains("Source"));
        assert!(bundled.contains("Bundled"));
        assert!(bundled.contains("Not synced yet."));

        let synced = Quiz {
            view: View::About,
            pack_origin: Some("Open Trivia DB".to_owned()),
            pack_updated_min: Some(1),
            ..Quiz::default()
        };
        let shown = format!("{:?}", screen(&synced));
        assert!(shown.contains("Open Trivia DB"));
        assert!(shown.contains("Synced "));
    }

    #[test]
    fn sync_stamps_origin_and_time() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("sync started");
        let item = r#"{"category":"Science","difficulty":"easy","question":"Q?","correct_answer":"Right","incorrect_answers":["W1","W2","W3"]}"#;
        let body = format!(
            r#"{{"response_code":0,"results":[{}]}}"#,
            [item; 10].join(",")
        );
        runner.task_outcome(task, TaskOutcome::Completed(body.into_bytes()));
        assert_eq!(runner.app().pack_origin.as_deref(), Some("Open Trivia DB"));
        assert!(runner.app().pack_updated_min.is_some());
        assert!(runner.app().state_line().contains("Open Trivia DB"));
    }

    #[test]
    fn age_labels_cover_minutes_hours_and_days() {
        assert_eq!(age_label(30), "just now");
        assert_eq!(age_label(5 * 60), "5 min ago");
        assert_eq!(age_label(3 * 3600), "3 hr ago");
        assert_eq!(age_label(2 * 86400), "2 days ago");
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
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("solo"));
        assert_eq!(runner.app().view, View::Setup);
        runner.action(action_id("continue-setup"));
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
    fn pass_around_renames_and_counts_players() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("count-2"));
        assert_eq!(runner.app().players, 2);
        runner.action(action_id("rename-0"));
        // Shift, s, a, m, then accept.
        for key in ["kb.shift", "kb.r1c1", "kb.r1c0", "kb.r2c6", "kb.enter"] {
            runner.action(action_id(key));
        }
        assert_eq!(runner.app().names[0], "Sam");
        assert!(!runner.app().entry.is_open());
        runner.action(action_id("start"));
        let correct = runner.app().round_questions[0].correct;
        runner.action(action_id(&choice(correct)));
        let pass = format!("{:?}", screen(runner.app()));
        assert!(pass.contains("Sam answered. Hand the Kobo to Bert"));
        runner.action(action_id("reveal"));
        runner.action(action_id("continue"));
        let correct = runner.app().round_questions[1].correct;
        runner.action(action_id(&choice(correct)));
        let pass = format!("{:?}", screen(runner.app()));
        // Two players: the turn comes back to Sam, never to Cleo or Dev.
        assert!(pass.contains("Hand the Kobo to Sam"));
        assert!(!pass.contains("Cleo"));
    }

    #[test]
    fn saved_state_carries_names_and_player_count() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(b"1|2|3|Sam,Bo,Cleo,Dev".to_vec()),
        });
        assert_eq!(runner.app().players, 3);
        assert_eq!(runner.app().names[0], "Sam");
        assert_eq!(runner.app().names[1], "Bo");
        // The pre-names format still loads, with defaults where it has nothing.
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(b"1|5".to_vec()),
        });
        assert_eq!(runner.app().rounds, 5);
        assert_eq!(runner.app().players, 3);
    }

    #[test]
    fn player_names_stay_short_and_separator_free() {
        assert_eq!(clean_name("  Sam "), Some("Sam".to_owned()));
        assert_eq!(
            clean_name("averylongnameindeed"),
            Some("averylongnam".to_owned())
        );
        assert_eq!(clean_name("a|b,c"), Some("abc".to_owned()));
        assert_eq!(clean_name(" | "), None);
    }

    #[test]
    fn zero_scoreboard_condenses_until_the_first_point() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("start"));
        let correct = runner.app().round_questions[0].correct;
        let wrong = (correct + 1) % 4;
        runner.action(action_id(&choice(wrong)));
        runner.action(action_id("reveal"));
        let condensed = format!("{:?}", screen(runner.app()));
        assert!(condensed.contains("No points yet."));
        assert!(!condensed.contains("0 points"));
        runner.action(action_id("continue"));
        let correct = runner.app().round_questions[1].correct;
        runner.action(action_id(&choice(correct)));
        runner.action(action_id("reveal"));
        let scored = format!("{:?}", screen(runner.app()));
        assert!(scored.contains("points"));
        assert!(!scored.contains("No points yet."));
    }

    #[test]
    fn late_cache_and_sync_do_not_replace_an_active_round() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("sync started");
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
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
        runner.action(action_id("continue-setup"));
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
