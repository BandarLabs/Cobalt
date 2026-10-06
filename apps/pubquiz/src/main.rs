//! Pub Quiz keeps its question packs and play state on the reader.

use kobo_json::Value;
use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::{
    action_id,
    keyboard::{TextEntry, Typing},
    ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult, Task,
    TaskId, TaskOutcome,
};
use std::fmt::Write;
use std::process::ExitCode;

const STATE: &str = "pubquiz-state";
const PACK: &str = "pubquiz-pack-v1";
const SCORECARD: &str = "pubquiz-scorecard.csv";
const LICENSE: &str = "pubquiz-content-license";
const LICENSE_TEXT: &str = "Questions: Open Trivia DB (opentdb.com), CC-BY-SA 4.0. Cached question content remains under CC-BY-SA 4.0.";
const API: &str = "https://opentdb.com/api.php?amount=50&type=multiple";
/// The longest round, and the fewest questions a round is worth starting with.
///
/// A synced pack spreads fifty questions over about twenty categories, so a
/// single category often holds one or two. A round of one question used to
/// start anyway and end on the podium before anybody but the first player had
/// held the reader, which looked like the game had broken.
const ROUND_LENGTH: usize = 10;
const MIN_ROUND: usize = 3;

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

/// The pack that plays before anything is synced.
///
/// Five questions in each of six categories, so any one category is a round
/// of its own and a first evening without Wi-Fi is three rounds rather than
/// one. Ten questions in nine categories made every category a round of one
/// or two questions.
#[allow(clippy::too_many_lines)]
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
            "Science",
            Difficulty::Easy,
            "What is the chemical symbol for gold?",
            ["Ag", "Gd", "Au", "Go"],
            2,
        ),
        question(
            "Science",
            Difficulty::Easy,
            "Which gas do plants take from the air to make sugar?",
            ["Oxygen", "Nitrogen", "Carbon dioxide", "Helium"],
            2,
        ),
        question(
            "Science",
            Difficulty::Medium,
            "How many bones are in the adult human body?",
            ["186", "206", "226", "246"],
            1,
        ),
        question(
            "Science",
            Difficulty::Medium,
            "What is the hardest natural mineral?",
            ["Quartz", "Topaz", "Corundum", "Diamond"],
            3,
        ),
        question(
            "Geography",
            Difficulty::Easy,
            "What is the capital of Finland?",
            ["Oslo", "Helsinki", "Tallinn", "Stockholm"],
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
            "Geography",
            Difficulty::Easy,
            "Which is the largest ocean?",
            ["Atlantic", "Indian", "Arctic", "Pacific"],
            3,
        ),
        question(
            "Geography",
            Difficulty::Medium,
            "What is the capital of Australia?",
            ["Sydney", "Melbourne", "Canberra", "Perth"],
            2,
        ),
        question(
            "Geography",
            Difficulty::Hard,
            "Which country has the most natural lakes?",
            ["Canada", "Russia", "Finland", "United States"],
            0,
        ),
        question(
            "History",
            Difficulty::Hard,
            "Which ship carried Charles Darwin on his voyage?",
            ["Beagle", "Endeavour", "Victory", "Resolution"],
            0,
        ),
        question(
            "History",
            Difficulty::Medium,
            "In which year did the Berlin Wall fall?",
            ["1987", "1989", "1991", "1993"],
            1,
        ),
        question(
            "History",
            Difficulty::Easy,
            "Who was the first person to walk on the Moon?",
            [
                "Buzz Aldrin",
                "Yuri Gagarin",
                "Neil Armstrong",
                "Michael Collins",
            ],
            2,
        ),
        question(
            "History",
            Difficulty::Medium,
            "Which civilisation built Machu Picchu?",
            ["Aztec", "Maya", "Inca", "Olmec"],
            2,
        ),
        question(
            "History",
            Difficulty::Medium,
            "Who was the first emperor of Rome?",
            ["Julius Caesar", "Augustus", "Nero", "Caligula"],
            1,
        ),
        question(
            "Arts & Literature",
            Difficulty::Medium,
            "Who painted The Persistence of Memory?",
            ["Miró", "Dalí", "Picasso", "Kahlo"],
            1,
        ),
        question(
            "Arts & Literature",
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
            "Arts & Literature",
            Difficulty::Easy,
            "How many strings does a standard violin have?",
            ["Three", "Four", "Five", "Six"],
            1,
        ),
        question(
            "Arts & Literature",
            Difficulty::Medium,
            "Who wrote One Hundred Years of Solitude?",
            [
                "Jorge Luis Borges",
                "Gabriel García Márquez",
                "Isabel Allende",
                "Pablo Neruda",
            ],
            1,
        ),
        question(
            "Arts & Literature",
            Difficulty::Medium,
            "In which city is the Rijksmuseum?",
            ["Brussels", "Amsterdam", "Copenhagen", "Vienna"],
            1,
        ),
        question(
            "Nature",
            Difficulty::Medium,
            "Which is the largest living bird?",
            ["Emu", "Albatross", "Ostrich", "Condor"],
            2,
        ),
        question(
            "Nature",
            Difficulty::Easy,
            "How many legs does a spider have?",
            ["Six", "Eight", "Ten", "Twelve"],
            1,
        ),
        question(
            "Nature",
            Difficulty::Easy,
            "What is a baby kangaroo called?",
            ["Cub", "Kit", "Joey", "Calf"],
            2,
        ),
        question(
            "Nature",
            Difficulty::Easy,
            "Which tree grows acorns?",
            ["Oak", "Beech", "Maple", "Ash"],
            0,
        ),
        question(
            "Nature",
            Difficulty::Easy,
            "Which is the fastest land animal?",
            ["Lion", "Pronghorn", "Cheetah", "Greyhound"],
            2,
        ),
        question(
            "Sport & Games",
            Difficulty::Easy,
            "How many players does a football team start with?",
            ["Nine", "Ten", "Eleven", "Twelve"],
            2,
        ),
        question(
            "Sport & Games",
            Difficulty::Easy,
            "How many squares are on a chessboard?",
            ["36", "49", "64", "81"],
            2,
        ),
        question(
            "Sport & Games",
            Difficulty::Easy,
            "In which sport does love mean a score of zero?",
            ["Golf", "Tennis", "Squash", "Cricket"],
            1,
        ),
        question(
            "Sport & Games",
            Difficulty::Medium,
            "How many points is the black ball worth in snooker?",
            ["Five", "Six", "Seven", "Eight"],
            2,
        ),
        question(
            "Sport & Games",
            Difficulty::Medium,
            "Which country hosted the first modern Olympic Games?",
            ["France", "Greece", "England", "United States"],
            1,
        ),
    ]
    .into()
}
#[allow(clippy::struct_excessive_bools)]
struct Quiz {
    view: View,
    party: bool,
    player: usize,
    players: usize,
    names: [String; 4],
    renaming: usize,
    entry: TextEntry,
    question: usize,
    /// What each player answered to the current question. Everyone answers
    /// the same question in turn, so the reveal can say who got it.
    answers: [Option<usize>; 4],
    scores: [u8; 4],
    /// A reader pressed Back mid-round and is being asked whether to leave.
    leaving: bool,
    packs: u8,
    note: Option<String>,
    rounds: u16,
    questions: Vec<Question>,
    round_questions: Vec<Question>,
    scorecard_saved: bool,
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
            answers: [None; 4],
            scores: [0; 4],
            leaving: false,
            packs: 0,
            note: None,
            rounds: 0,
            questions: bundled_questions(),
            round_questions: bundled_questions(),
            scorecard_saved: false,
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
    fn scorecard_csv(&self) -> String {
        let players = if self.party { self.players } else { 1 };
        let mut csv = "round,player,points\n".to_string();
        for index in 0..players {
            let _ = writeln!(
                csv,
                "{},{},{}",
                self.rounds,
                self.name_for(index),
                self.scores[index]
            );
        }
        csv
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
        if let Some(&(_, difficulty)) = DIFFICULTIES
            .iter()
            .find(|(name, _)| action == action_id(name))
        {
            self.setup_difficulty = difficulty;
        } else if action == action_id("cat-any") {
            self.setup_category = None;
        } else if action == action_id("next-page") {
            let last = setup_pages(self, context).len().saturating_sub(1);
            self.setup_page = (self.setup_page.min(last) + 1).min(last);
        } else if action == action_id("previous-page") {
            let last = setup_pages(self, context).len().saturating_sub(1);
            self.setup_page = self.setup_page.min(last).saturating_sub(1);
        } else if action == action_id("continue-setup") {
            if self.setup_party {
                self.view = View::Players;
            } else {
                self.begin(false);
            }
        } else if action != ActionId::BACK && action != action_id("home") {
            let categories = setup_categories(self);
            let Some(index) =
                (0..categories.len()).find(|i| action == action_id(&format!("cat-{i}")))
            else {
                return false;
            };
            if let Some(name) = categories.get(index) {
                self.setup_category = Some(name.clone());
            }
        } else {
            return false;
        }
        true
    }

    /// The questions the current setup would deal from.
    fn pool(&self, category: Option<&str>) -> Vec<Question> {
        self.questions
            .iter()
            .filter(|question| {
                category.is_none_or(|category| question.category == category)
                    && self
                        .setup_difficulty
                        .is_none_or(|difficulty| question.difficulty == difficulty)
            })
            .cloned()
            .collect()
    }

    fn begin(&mut self, party: bool) {
        let pool = self.pool(self.setup_category.as_deref());
        if pool.len() < MIN_ROUND {
            self.note = Some(if pool.is_empty() {
                "No questions match that mix yet. Sync packs or widen the choice.".to_owned()
            } else {
                format!(
                    "Only {} matching. Choose Any category or sync packs for a full round.",
                    questions_label(pool.len())
                )
            });
            self.view = View::Home;
            return;
        }
        self.party = party;
        self.scorecard_saved = false;
        self.view = View::Question;
        self.question = 0;
        self.player = 0;
        self.answers = [None; 4];
        self.scores = [0; 4];
        self.leaving = false;
        self.note = None;
        self.page = 0;
        let offset = usize::from(self.rounds) * ROUND_LENGTH % pool.len();
        let length = pool.len().min(ROUND_LENGTH);
        self.round_questions = pool
            .iter()
            .cycle()
            .skip(offset)
            .take(length)
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
    fn rename_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == ActionId::BACK && self.entry.is_open() {
            self.entry.close();
            self.show(context);
            return true;
        }
        if let Some(event) = self.entry.handle(action) {
            if let Typing::Submitted(name) = event {
                if let Some(name) = clean_name(&name) {
                    self.names[self.renaming] = name;
                    self.save(context);
                }
            }
            self.show(context);
            return true;
        }
        false
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(screen_with(self, context));
    }
}
/// The difficulty segments, each its own action so a tap names its value.
const DIFFICULTIES: [(&str, Option<Difficulty>); 4] = [
    ("diff-any", None),
    ("diff-easy", Some(Difficulty::Easy)),
    ("diff-medium", Some(Difficulty::Medium)),
    ("diff-hard", Some(Difficulty::Hard)),
];

fn questions_label(count: usize) -> String {
    if count == 1 {
        "1 question".to_owned()
    } else {
        format!("{count} questions")
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

fn points_label(points: u8) -> String {
    if points == 1 {
        "1 point".to_owned()
    } else {
        format!("{points} points")
    }
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
    let mut text = format!("{}\n\n{}", round_position(quiz, question), question.text);
    for (index, answer) in question.answers.iter().enumerate() {
        write!(text, "\n\n{} · {answer}", index + 1).expect("writing to a String");
    }
    text
}

fn question_title(quiz: &Quiz) -> String {
    if quiz.party {
        format!("{}'s turn", quiz.player_name())
    } else {
        "Solo round".into()
    }
}

/// The round's place, said the same way on every screen of it.
fn round_position(quiz: &Quiz, question: &Question) -> String {
    format!(
        "{} · Question {} of {}",
        question.category,
        quiz.question + 1,
        quiz.round_questions.len()
    )
}

/// The hand-over between players, which shows nothing of the round.
///
/// It used to come after an answer and before that answer's result, so the
/// next player was handed somebody else's result to reveal. Every player now
/// answers the same question, so the one thing this screen has to protect is
/// the previous answer, and it names only whose turn it is.
fn pass_screen(quiz: &Quiz) -> Screen {
    let next = quiz.player_name().to_owned();
    let previous = quiz.name_for(quiz.player.saturating_sub(1)).to_owned();
    ScreenBuilder::new("pubquiz-pass")
        .top_bar(format!(
            "Question {} of {}",
            quiz.question + 1,
            quiz.round_questions.len()
        ))
        .owns_back(true)
        .splash(
            Some(Glyph::Person),
            format!("Pass to {next}"),
            format!("{previous} has answered. {next}, tap below when only you can see the screen."),
        )
        .bottom_action("show-question", format!("Show {next}'s question"))
        .compose(|b| leave_prompt(quiz, b))
        .build()
}

fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn podium_screen(quiz: &Quiz) -> Screen {
    let count = if quiz.party { quiz.players } else { 1 };
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(quiz.scores[i]));
    let best = quiz.scores[order[0]];
    let winners: Vec<&str> = order
        .iter()
        .filter(|&&i| quiz.scores[i] == best)
        .map(|&i| quiz.name_for(i))
        .collect();
    let headline = if !quiz.party {
        format!("{best} of {}", quiz.round_questions.len())
    } else if winners.len() == count && count > 1 {
        "A draw".to_owned()
    } else if winners.len() == 1 {
        format!("{} wins", winners[0])
    } else {
        format!("{} share the win", join_names(&winners))
    };
    // Places, not a list in score order with nothing to say who came first:
    // tied players share a place, the way a pub quiz master would read it out.
    let mut place = 0;
    let mut previous = None;
    let rows: Vec<_> = order
        .iter()
        .enumerate()
        .map(|(position, &i)| {
            if previous != Some(quiz.scores[i]) {
                place = position + 1;
                previous = Some(quiz.scores[i]);
            }
            (
                format!("player-{i}"),
                quiz.name_for(i).to_owned(),
                String::new(),
                u16::try_from(place).unwrap_or(1),
                points_label(quiz.scores[i]),
            )
        })
        .collect();
    ScreenBuilder::new("pubquiz-podium")
        .top_bar("Results")
        .top_bar_action(
            "save-scorecard",
            if quiz.scorecard_saved {
                "Saved"
            } else {
                "Save scores"
            },
        )
        .secondary(format!(
            "Round {} · {}",
            quiz.rounds,
            questions_label(quiz.round_questions.len())
        ))
        .heading(headline)
        .rows_with_trailing(rows)
        .primary_button("play-again", "Play again")
        .button("home", "Done")
        .build()
}

fn home_screen(quiz: &Quiz) -> Screen {
    let mut b = ScreenBuilder::new("pubquiz-home")
        .top_bar("Pub Quiz")
        .heading("Ready when you are")
        .secondary(format!(
            "{} · {} pack · {} rounds played",
            questions_label(quiz.questions.len()),
            quiz.source_label(),
            quiz.rounds
        ));
    if let Some(note) = &quiz.note {
        b = b.banner(BannerLevel::Info, note);
    }
    b.primary_button("party", "Play with friends")
        .button("solo", "Play solo")
        .section("More")
        .rows([
            (
                "sync",
                "Get more questions",
                if quiz.sync_task.is_some() {
                    "Downloading from Open Trivia DB…".to_owned()
                } else {
                    format!("{} · Needs Wi-Fi once", quiz.updated_label())
                },
                Glyph::Download,
            ),
            (
                "how-to-play",
                "How to play",
                "Solo or passed around the table".to_owned(),
                Glyph::Note,
            ),
            (
                "about",
                "About the questions",
                "Open Trivia DB, CC BY-SA 4.0".to_owned(),
                Glyph::Book,
            ),
        ])
        .build()
}

/// Asks before Back throws a round away.
///
/// One press of Back mid-round used to drop every score on the table without a
/// word. Every screen of a round owns Back and routes it here instead.
fn leave_prompt(quiz: &Quiz, builder: ScreenBuilder) -> ScreenBuilder {
    if quiz.leaving {
        builder.confirm(
            "Leave this round?",
            "The scores so far will be lost.",
            ("leave-round", "Leave round"),
            ("keep-playing", "Keep playing"),
        )
    } else {
        builder
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
    // The question in the display serif, so it reads as the card a quiz
    // master holds up rather than as one more line of interface.
    let compact = || {
        ScreenBuilder::new("pubquiz-question")
            .top_bar(question_title(quiz))
            .owns_back(true)
            .secondary(round_position(quiz, question))
            .heading_at_level(2, &question.text)
            .rows(answer_rows(question))
    };
    // Measured without the leave prompt: a dialogue over the question is not
    // the question failing to fit, and treating it as one swapped the page
    // behind it for the paginated layout.
    if compact()
        .build()
        .diagnostics(&context.metrics(), &kobo_sdk::Chrome::default())
        .issues
        .is_empty()
    {
        return compact().compose(|b| leave_prompt(quiz, b)).build();
    }
    // Long questions stay complete, including answers that share a prefix.
    // The same measured prose pagination as the reader keeps every word reachable.
    let pages = context.paginate(&question_text(quiz), true);
    let page = quiz.page.min(pages.len().saturating_sub(1));
    let mut builder = ScreenBuilder::new("pubquiz-question")
        .top_bar(question_title(quiz))
        .owns_back(true);
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
        .compose(|b| leave_prompt(quiz, b))
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
        .owns_back(true)
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
        .compose(|b| leave_prompt(quiz, b))
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

fn setup_prefix(quiz: &Quiz) -> ScreenBuilder {
    let selected = DIFFICULTIES
        .iter()
        .position(|&(_, difficulty)| difficulty == quiz.setup_difficulty)
        .unwrap_or(0);
    let matching = quiz.pool(quiz.setup_category.as_deref()).len();
    ScreenBuilder::new("pubquiz-setup")
        .top_bar(if quiz.setup_party {
            "Pass-around round"
        } else {
            "Solo round"
        })
        .owns_back(true)
        .section("Difficulty")
        .segmented(
            selected,
            DIFFICULTIES
                .iter()
                .map(|&(name, difficulty)| (name, difficulty.map_or("Any", Difficulty::label))),
        )
        // How many questions the choice leaves, before Continue rather than
        // after: a category of one question is visible as one here instead of
        // as a round that ends as soon as it starts.
        .section_with_value("Category", format!("{} match", questions_label(matching)))
}

fn setup_rows(quiz: &Quiz, context: &Context) -> Vec<(String, String, String, Glyph, String)> {
    let mark = |chosen: bool| if chosen { Glyph::Check } else { Glyph::Circle };
    let mut rows = vec![(
        "cat-any".to_owned(),
        "Any category".to_owned(),
        String::new(),
        mark(quiz.setup_category.is_none()),
        quiz.pool(None).len().to_string(),
    )];
    rows.extend(
        setup_categories(quiz)
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let count = quiz.pool(Some(&name)).len().to_string();
                (
                    format!("cat-{index}"),
                    context.clamped_row(&name, 2, true),
                    String::new(),
                    mark(quiz.setup_category.as_ref() == Some(&name)),
                    count,
                )
            }),
    );
    rows
}

fn setup_pages(quiz: &Quiz, context: &Context) -> Vec<Vec<usize>> {
    let rows = setup_rows(quiz, context);
    let measured: Vec<_> = rows
        .iter()
        .map(|(_, title, detail, _, _)| (title.as_str(), detail.as_str()))
        .collect();
    context.paginate_rows_under(
        &measured,
        true,
        kobo_sdk::Position::AtTheFoot,
        &setup_prefix(quiz).build(),
    )
}

fn setup_screen(quiz: &Quiz, context: &Context) -> Screen {
    let rows = setup_rows(quiz, context);
    let pages = setup_pages(quiz, context);
    let page = quiz.setup_page.min(pages.len().saturating_sub(1));
    let mut builder =
        setup_prefix(quiz).rows_with_trailing(pages[page].iter().map(|&index| rows[index].clone()));
    if pages.len() > 1 {
        builder = builder
            .page_turns("previous-page", "next-page")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
    }
    builder
        .bottom_action(
            "continue-setup",
            if quiz.setup_party {
                "Choose players"
            } else {
                "Start round"
            },
        )
        .build()
}

fn players_screen(quiz: &Quiz) -> Screen {
    ScreenBuilder::new("pubquiz-players")
        .top_bar("Players")
        .owns_back(true)
        .section("How many")
        .segmented(
            quiz.players.saturating_sub(2),
            [("count-2", "2"), ("count-3", "3"), ("count-4", "4")],
        )
        .section_with_value("Names", "Tap a name to rename")
        .grid(
            2,
            false,
            (0..quiz.players).map(|i| (format!("rename-{i}"), quiz.names[i].as_str())),
        )
        .bottom_action("start", "Start round")
        .build()
}

fn reveal_screen(quiz: &Quiz, question: &Question) -> Screen {
    let players = if quiz.party { quiz.players } else { 1 };
    let right: Vec<&str> = (0..players)
        .filter(|&i| quiz.answers[i] == Some(question.correct))
        .map(|i| quiz.name_for(i))
        .collect();
    let headline = match (quiz.party, right.len()) {
        (false, 0) => "Not this time".to_owned(),
        (false, _) => "Correct".to_owned(),
        (true, 0) => "Nobody got it".to_owned(),
        (true, n) if n == players => "Everyone got it".to_owned(),
        (true, _) => format!("{} got it", join_names(&right)),
    };
    // Each answer with who chose it, and the right one ticked: the arguing
    // screen, where the table can see exactly how everyone went wrong.
    let answers = question.answers.iter().enumerate().map(|(index, answer)| {
        let chose: Vec<&str> = (0..players)
            .filter(|&i| quiz.answers[i] == Some(index))
            .map(|i| quiz.name_for(i))
            .collect();
        (
            format!("reveal-{index}"),
            answer.clone(),
            if index == question.correct {
                "Right answer".to_owned()
            } else {
                String::new()
            },
            if index == question.correct {
                Glyph::Check
            } else {
                Glyph::Circle
            },
            if quiz.party {
                join_names(&chose)
            } else if chose.is_empty() {
                String::new()
            } else {
                "Your answer".to_owned()
            },
        )
    });
    let mut builder = ScreenBuilder::new("pubquiz-reveal")
        .top_bar(format!(
            "Question {} of {}",
            quiz.question + 1,
            quiz.round_questions.len()
        ))
        .owns_back(true)
        .secondary(&question.category)
        .heading(headline)
        .text(&question.text)
        .rows_with_trailing(answers);
    if quiz.party {
        builder = builder
            .section("Scores")
            .facts((0..players).map(|i| (quiz.name_for(i), points_label(quiz.scores[i]))));
    } else {
        builder = builder.section_with_value(
            "Score",
            format!("{} of {}", quiz.scores[0], quiz.question + 1),
        );
    }
    builder
        .bottom_action(
            "continue",
            if quiz.question + 1 == quiz.round_questions.len() {
                "See results"
            } else {
                "Next question"
            },
        )
        .compose(|b| leave_prompt(quiz, b))
        .build()
}

fn screen_with(quiz: &Quiz, context: &Context) -> Screen {
    if quiz.entry.is_open() {
        return ScreenBuilder::new("pubquiz-rename")
            .top_bar("Rename player")
            .owns_back(true)
            .secondary("Letters and digits, twelve characters or fewer.")
            .text_entry(&quiz.entry, "Player name", "Done")
            .build();
    }
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    match quiz.view {
        View::Setup => setup_screen(quiz, context),
        View::Players => players_screen(quiz),
        View::Home => home_screen(quiz),
        View::Question => question_screen(quiz, context),
        View::Choices => choices_screen(quiz, context),
        View::Pass => pass_screen(quiz),
        View::Reveal => reveal_screen(quiz, question),
        View::Podium => podium_screen(quiz),
        View::HowTo => ScreenBuilder::new("pubquiz-help")
            .top_bar("How to play")
            .owns_back(true)
            .section("Solo")
            .text("Ten questions. See each answer straight away.")
            .section("With friends")
            .text("Two to four players, one reader. Each question goes round the table, then everyone sees who was right.")
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
    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Setup && !self.entry.is_open() {
            self.on_action(
                context,
                action_id(if forward {
                    "next-page"
                } else {
                    "previous-page"
                }),
            );
        }
    }
    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.rename_action(context, action) {
            return;
        }
        let in_round = matches!(
            self.view,
            View::Question | View::Choices | View::Pass | View::Reveal
        );
        if self.leaving {
            // The confirmation is the only thing that answers while it is up.
            if action == action_id("leave-round") {
                self.leaving = false;
                self.view = View::Home;
            } else if action == action_id("keep-playing") || action == ActionId::BACK {
                self.leaving = false;
            }
        } else if action == ActionId::BACK && in_round {
            self.leaving = true;
        } else if action == ActionId::BACK && self.view == View::Players {
            self.view = View::Setup;
        } else if action == action_id("choose") && self.view == View::Question {
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
        } else if let Some(count) = (self.view == View::Players)
            .then(|| (2..=4).find(|n| action == action_id(&format!("count-{n}"))))
            .flatten()
        {
            self.players = count;
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
        } else if action == action_id("save-scorecard") && self.view == View::Podium {
            context
                .store()
                .save(SCORECARD, self.scorecard_csv().into_bytes());
            self.scorecard_saved = true;
        } else if action == action_id("play-again") && self.view == View::Podium {
            self.begin(self.party);
        } else if action == action_id("about") && self.view == View::Home {
            self.view = View::About;
        } else if action == action_id("how-to-play") && self.view == View::Home {
            self.view = View::HowTo;
        } else if action == ActionId::BACK || action == action_id("home") {
            self.view = View::Home;
        } else if let Some(answer) = (0..4).find(|i| {
            matches!(self.view, View::Question | View::Choices) && action == action_id(&choice(*i))
        }) {
            self.answers[self.player] = Some(answer);
            if answer == self.round_questions[self.question % self.round_questions.len()].correct {
                self.scores[self.player] += 1;
            }
            self.page = 0;
            // Round the table, then everyone sees the answer together.
            if self.party && self.player + 1 < self.players {
                self.player += 1;
                self.view = View::Pass;
            } else {
                self.view = View::Reveal;
            }
        } else if action == action_id("show-question") && self.view == View::Pass {
            self.view = View::Question;
        } else if action == action_id("continue") && self.view == View::Reveal {
            self.question += 1;
            self.player = 0;
            self.page = 0;
            self.answers = [None; 4];
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
    fn the_hand_over_shows_neither_the_question_nor_the_last_answer() {
        let mut quiz = Quiz::default();
        quiz.begin(true);
        quiz.answers[0] = Some(0);
        quiz.player = 1;
        quiz.view = View::Pass;
        let shown = screen(&quiz);
        let layout = shown.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("show-question")).is_some());
        assert!(layout.rect_of_action(action_id("answer-0")).is_none());
        let text = format!("{shown:?}");
        assert!(text.contains("Pass to Bert"));
        assert!(!text.contains(&quiz.round_questions[0].text));
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
        assert_eq!(bundled_questions().len(), 30);
        // Every bundled category is a round of its own.
        let quiz = Quiz::default();
        for category in setup_categories(&quiz) {
            assert!(quiz.pool(Some(&category)).len() >= MIN_ROUND, "{category}");
        }
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
        // Four players: each answers the same question, with a hand-over
        // between them, and only then does the round reveal it.
        for _ in 0..3 {
            runner.action(action_id(&choice(0)));
            assert_eq!(runner.app().view, View::Pass);
            runner.action(action_id("show-question"));
            assert_eq!(runner.app().view, View::Question);
        }
        runner.action(action_id(&choice(0)));
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
        runner.action(action_id("diff-easy"));
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
    fn short_pools_deal_short_rounds_without_repeats() {
        let mut quiz = Quiz {
            setup_category: Some("Nature".to_owned()),
            ..Quiz::default()
        };
        let pool: Vec<_> = quiz
            .questions
            .iter()
            .filter(|question| question.category == "Nature")
            .cloned()
            .collect();
        assert!(pool.len() < 10, "test needs a small category");
        quiz.begin(false);
        assert_eq!(quiz.round_questions.len(), pool.len());
        let mut texts: Vec<_> = quiz
            .round_questions
            .iter()
            .map(|q| q.text.clone())
            .collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), pool.len());
    }

    #[test]
    fn the_reveal_ticks_the_right_answer_and_says_who_chose_what() {
        let mut quiz = Quiz {
            players: 3,
            ..Quiz::default()
        };
        quiz.begin(true);
        let question = quiz.round_questions[0].clone();
        let wrong = (question.correct + 1) % 4;
        quiz.answers = [Some(question.correct), Some(wrong), Some(wrong), None];
        let shown = format!("{:?}", reveal_screen(&quiz, &question));
        assert!(shown.contains("Ada got it"));
        assert!(shown.contains("Right answer"));
        assert!(shown.contains("Bert and Cleo"));
        assert!(shown.contains(&format!("Question 1 of {}", quiz.round_questions.len())));

        quiz.answers = [Some(wrong); 4];
        let shown = format!("{:?}", reveal_screen(&quiz, &question));
        assert!(shown.contains("Nobody got it"));

        let mut solo = Quiz::default();
        solo.begin(false);
        let question = solo.round_questions[0].clone();
        solo.answers[0] = Some(question.correct);
        let shown = format!("{:?}", reveal_screen(&solo, &question));
        assert!(shown.contains("Correct"));
        assert!(shown.contains("Your answer"));
    }

    #[test]
    fn the_last_question_offers_the_podium() {
        let mut quiz = Quiz {
            setup_category: Some("Nature".to_owned()),
            ..Quiz::default()
        };
        quiz.begin(false);
        quiz.question = quiz.round_questions.len() - 1;
        let question = quiz.round_questions[quiz.question].clone();
        quiz.answers[0] = Some(question.correct);
        let shown = format!("{:?}", reveal_screen(&quiz, &question));
        assert!(shown.contains("See results"));
        let mut full = Quiz::default();
        full.begin(false);
        let first = full.round_questions[0].clone();
        full.answers[0] = Some(first.correct);
        let shown = format!("{:?}", reveal_screen(&full, &first));
        assert!(shown.contains("Next question"));
    }

    #[test]
    fn scorecard_csv_lists_the_round_and_scores() {
        let mut quiz = Quiz {
            party: true,
            players: 2,
            rounds: 3,
            ..Quiz::default()
        };
        quiz.scores = [7, 5, 0, 0];
        let csv = quiz.scorecard_csv();
        assert!(csv.starts_with("round,player,points\n"));
        assert!(csv.contains("3,Ada,7\n"));
        assert!(csv.contains("3,Bert,5\n"));
        assert!(!csv.contains("Cleo"));

        let solo = Quiz {
            party: false,
            rounds: 1,
            ..Quiz::default()
        };
        assert!(solo.scorecard_csv().contains("1,You,0\n"));
    }

    #[test]
    fn the_podium_offers_a_scorecard_once() {
        use kobo_sdk::AppRunner;
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("start"));
        runner.app_mut().question = runner.app().round_questions.len() - 1;
        let correct = runner.app().round_questions[runner.app().question].correct;
        for _ in 0..3 {
            runner.action(action_id(&choice(correct)));
            runner.action(action_id("show-question"));
        }
        runner.action(action_id(&choice(correct)));
        runner.action(action_id("continue"));
        assert_eq!(runner.app().view, View::Podium);
        let shown = format!("{:?}", screen(runner.app()));
        assert!(shown.contains("Save scores"));
        // Everyone scored the one point: a four-way draw, all in first place.
        assert!(shown.contains("A draw"));
        runner.action(action_id("save-scorecard"));
        assert!(runner.app().scorecard_saved);
        let shown = format!("{:?}", screen(runner.app()));
        assert!(shown.contains("Saved"));
    }

    #[test]
    fn points_are_pluralized() {
        assert_eq!(points_label(0), "0 points");
        assert_eq!(points_label(1), "1 point");
        assert_eq!(points_label(7), "7 points");
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
        assert!(pass.contains("Sam has answered."));
        assert!(pass.contains("Pass to Bert"));
        runner.action(action_id("show-question"));
        assert!(format!("{:?}", screen(runner.app())).contains("Bert's turn"));
        runner.action(action_id(&choice(correct)));
        // Two players: after Bert the question is revealed, never handed to
        // Cleo or Dev, and the next one starts with Sam again.
        assert_eq!(runner.app().view, View::Reveal);
        assert!(!format!("{:?}", screen(runner.app())).contains("Cleo"));
        runner.action(action_id("continue"));
        assert!(format!("{:?}", screen(runner.app())).contains("Sam's turn"));
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
    fn every_player_answers_every_question_so_turns_are_even() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz {
            players: 3,
            ..Quiz::default()
        });
        runner.start();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("start"));
        let total = runner.app().round_questions.len();
        for _ in 0..total {
            let correct = runner.app().round_questions[runner.app().question].correct;
            // Ada always right, Bert always wrong, Cleo right.
            for (player, answer) in [correct, (correct + 1) % 4, correct]
                .into_iter()
                .enumerate()
            {
                assert_eq!(runner.app().player, player);
                runner.action(action_id(&choice(answer)));
                if player < 2 {
                    runner.action(action_id("show-question"));
                }
            }
            assert_eq!(runner.app().view, View::Reveal);
            runner.action(action_id("continue"));
        }
        let points = u8::try_from(total).unwrap();
        assert_eq!(runner.app().scores, [points, 0, points, 0]);
        assert_eq!(runner.app().view, View::Podium);
        assert!(format!("{:?}", screen(runner.app())).contains("Ada and Cleo share the win"));
    }

    #[test]
    fn back_mid_round_asks_before_throwing_the_scores_away() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        runner.action(ActionId::BACK);
        assert!(runner.app().leaving);
        assert_eq!(runner.app().view, View::Question);
        assert!(format!("{:?}", screen(runner.app())).contains("Leave this round?"));
        runner.action(action_id("keep-playing"));
        assert!(!runner.app().leaving);
        assert_eq!(runner.app().view, View::Question);
        runner.action(ActionId::BACK);
        runner.action(action_id("leave-round"));
        assert_eq!(runner.app().view, View::Home);
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
    fn full_rounds_advance_and_each_round_starts_fresh() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        for round in 1..=2u16 {
            runner.action(action_id("solo"));
            runner.action(action_id("continue-setup"));
            assert_eq!(runner.app().view, View::Question);
            assert_eq!(runner.app().question, 0);
            assert_eq!(runner.app().scores, [0, 0, 0, 0]);
            let total = runner.app().round_questions.len();
            for _ in 0..total {
                let correct = runner.app().round_questions[runner.app().question].correct;
                runner.action(action_id(&choice(correct)));
                runner.action(action_id("continue"));
            }
            assert_eq!(runner.app().view, View::Podium);
            assert_eq!(runner.app().rounds, round);
            assert_eq!(runner.app().scores[0], u8::try_from(total).unwrap());
            runner.action(action_id("home"));
            assert_eq!(runner.app().view, View::Home);
        }
        assert_eq!(runner.app().rounds, 2);
    }

    #[test]
    fn consecutive_rounds_do_not_repeat_while_the_pool_allows() {
        let mut quiz = Quiz {
            questions: (0..25)
                .map(|index| Question {
                    category: "Generated".to_owned(),
                    difficulty: Difficulty::Easy,
                    text: format!("Generated question {index}?"),
                    answers: ["A".into(), "B".into(), "C".into(), "D".into()],
                    correct: 0,
                })
                .collect(),
            ..Quiz::default()
        };
        quiz.begin(false);
        let first: Vec<String> = quiz
            .round_questions
            .iter()
            .map(|question| question.text.clone())
            .collect();
        quiz.rounds = 1;
        quiz.begin(false);
        let second: Vec<String> = quiz
            .round_questions
            .iter()
            .map(|question| question.text.clone())
            .collect();
        for round in [&first, &second] {
            let mut sorted = round.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), 10, "no repeats inside a round");
        }
        assert!(
            first.iter().all(|text| !second.contains(text)),
            "round two draws fresh questions while the pool has them"
        );
        // Once the offset wraps the pool, questions repeat across rounds,
        // but never inside one.
        quiz.rounds = 4;
        quiz.begin(false);
        let mut wrapped: Vec<String> = quiz
            .round_questions
            .iter()
            .map(|question| question.text.clone())
            .collect();
        wrapped.sort();
        wrapped.dedup();
        assert_eq!(wrapped.len(), 10);
    }

    fn only_spawn(commands: &[kobo_sdk::Command]) -> TaskId {
        let spawned: Vec<TaskId> = commands
            .iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .collect();
        assert_eq!(spawned.len(), 1);
        spawned[0]
    }

    // A retrying fetch gets a quiet second chance: the first offline failure
    // naps, then the retry runs. Only the second failure reaches the app.
    fn fail_sync_after_retry(runner: &mut kobo_sdk::AppRunner<Quiz>, task: TaskId) {
        let nap = only_spawn(
            &runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Offline)),
        );
        assert_eq!(
            runner.app().sync_task,
            Some(task),
            "the first offline failure naps instead of alarming the reader"
        );
        let retry = only_spawn(&runner.task_outcome(nap, TaskOutcome::Completed(Vec::new())));
        runner.task_outcome(retry, TaskOutcome::Failed(kobo_sdk::TaskError::Offline));
    }

    #[test]
    fn offline_sync_keeps_existing_packs_playing() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("sync started");
        let before = runner.app().questions.clone();
        fail_sync_after_retry(&mut runner, task);
        assert!(runner.app().sync_task.is_none());
        assert_eq!(
            runner.app().note.as_deref(),
            Some("Off the air. Existing packs still play offline.")
        );
        assert_eq!(runner.app().questions, before);
        assert!(runner.app().pack_updated_min.is_none());
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        assert_eq!(runner.app().round_questions.len(), 10);
    }

    #[test]
    fn a_refreshed_pack_survives_the_next_offline_sync() {
        let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("sync started");
        runner.task_outcome(task, TaskOutcome::Completed(pack()));
        assert_eq!(runner.app().questions[0].category, "New pack");
        let synced = runner.app().pack_updated_min;
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("second sync started");
        fail_sync_after_retry(&mut runner, task);
        assert_eq!(runner.app().questions[0].category, "New pack");
        assert_eq!(runner.app().pack_origin.as_deref(), Some("Open Trivia DB"));
        assert_eq!(runner.app().pack_updated_min, synced);
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

#[cfg(test)]
mod setup_tests;
