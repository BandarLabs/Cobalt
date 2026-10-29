//! Pub Quiz keeps its question packs and play state on the reader.

mod bank;
mod round;

use kobo_json::Value;
use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::failure::{Problem, Service};
use kobo_sdk::game::{self, Menu, Tap};
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
const LICENSE_TEXT: &str = "Downloaded questions: Open Trivia DB (opentdb.com), CC BY-SA 4.0. Cached downloaded packs remain under CC BY-SA 4.0. The bundled questions were written for Pub Quiz.";
/// The menu entry that opens About from a round.
const ABOUT: &str = "about";
/// The confirmed form of New game, which ends the round.
const NEW_GAME_CONFIRMED: &str = "new-game-confirmed";
const API: &str = "https://opentdb.com/api.php?amount=50&type=multiple";
/// The longest round, and the fewest questions a round is worth starting with.
///
/// A synced pack spreads fifty questions over about twenty categories, so a
/// single category often holds one or two. A round of one question used to
/// start anyway and end on the podium before anybody but the first player had
/// held the reader, which looked like the game had broken.
const ROUND_LENGTH: usize = 10;
const MIN_ROUND: usize = 3;
/// The name Open Trivia DB's failures are told in.
const SOURCE: Service<'static> = Service::new("Open Trivia DB");

/// A message on Home, kept with how it is drawn.
///
/// A fresh pack and a failed one used to be the same grey banner, and the
/// only retry was a row further down under another name. Each message now
/// carries its level, and a failure carries the control that answers it.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Note {
    /// Said at its level. `retry` puts Try again on the banner, which asks
    /// Open Trivia DB again.
    Said {
        level: BannerLevel,
        text: String,
        retry: bool,
    },
    /// A sync that failed, drawn with the one control that may fix it: Join
    /// Wi-Fi offline, Try again where asking again may help.
    Problem(Problem),
}
impl Note {
    fn said(level: BannerLevel, text: impl Into<String>) -> Self {
        Self::Said {
            level,
            text: text.into(),
            retry: false,
        }
    }
    #[cfg(test)]
    fn text(&self) -> &str {
        match self {
            Self::Said { text, .. } => text,
            Self::Problem(problem) => &problem.message,
        }
    }
    #[cfg(test)]
    fn level(&self) -> BannerLevel {
        match self {
            Self::Said { level, .. } => *level,
            Self::Problem(problem) => problem.level(),
        }
    }
}

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
    Rules,
    About,
}
impl View {
    /// The screens a round is played on, where Back asks before leaving.
    fn in_round(self) -> bool {
        matches!(
            self,
            Self::Question | Self::Choices | Self::Pass | Self::Reveal
        )
    }
}

/// What the dialogue over a round is asking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Confirm {
    /// Back was pressed mid-round.
    Leave,
    /// New game was chosen from the menu mid-round.
    NewGame,
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
    /// The spelling the data file and the saved round use.
    fn key(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
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

#[cfg(test)]
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

/// A synced pack added to the bundled questions, never in place of them.
///
/// A pack is fifty questions and the bundled set is several hundred, so
/// replacing one with the other made "Get more questions" leave fewer. The
/// pack goes first so its categories lead the first round dealt from it.
fn with_bundled(mut synced: Vec<Question>) -> Vec<Question> {
    let known: std::collections::HashSet<String> = synced
        .iter()
        .map(|question| question.text.to_lowercase())
        .collect();
    synced.extend(
        bank::bundled()
            .iter()
            .filter(|question| !known.contains(&question.text.to_lowercase()))
            .cloned(),
    );
    synced
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
    /// A dialogue over the round, waiting for an answer.
    confirm: Option<Confirm>,
    /// The game menu behind the dots in a round's top bar.
    menu: Menu,
    /// Where Rules and About go back to: home, or the round they were opened
    /// from.
    return_to: View,
    /// The round as last written to the store, so an action that changes
    /// nothing about it writes nothing.
    saved_round: Option<String>,
    packs: u8,
    note: Option<Note>,
    /// Why the last Continue on Setup did not start a round, said on Setup
    /// where the choice is made.
    setup_note: Option<String>,
    /// Whether the store refused the scorecard, so the podium says so and
    /// offers to save it again rather than claiming it is saved.
    scorecard_refused: bool,
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
            confirm: None,
            menu: Menu::default(),
            return_to: View::Home,
            saved_round: None,
            packs: 0,
            note: None,
            setup_note: None,
            scorecard_refused: false,
            rounds: 0,
            questions: bank::bundled().to_vec(),
            round_questions: bank::bundled()[..ROUND_LENGTH].to_vec(),
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
    fn source_label(&self) -> String {
        self.pack_origin.as_deref().map_or_else(
            || "Bundled".to_owned(),
            |origin| format!("Bundled and {origin}"),
        )
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
        self.setup_note = None;
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
            // Said on Setup, where the category and difficulty are chosen,
            // rather than on Home, a step away from anything that fixes it.
            self.setup_note = Some(if pool.is_empty() {
                "No questions match. Widen the mix.".to_owned()
            } else {
                format!(
                    "Only {} match. Widen the mix for a full round.",
                    questions_label(pool.len())
                )
            });
            self.setup_party = party;
            self.view = View::Setup;
            return;
        }
        self.setup_note = None;
        self.party = party;
        self.scorecard_saved = false;
        self.scorecard_refused = false;
        self.view = View::Question;
        self.question = 0;
        self.player = 0;
        self.answers = [None; 4];
        self.scores = [0; 4];
        self.confirm = None;
        self.menu.close();
        self.note = None;
        self.page = 0;
        let pool = bank::dealt(pool);
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

    /// Ends the round on screen and goes back to its choices, keeping the
    /// mode, category and difficulty it was dealt with.
    fn new_game(&mut self) {
        self.confirm = None;
        self.menu.close();
        self.setup_party = self.party;
        self.setup_page = 0;
        self.page = 0;
        self.view = View::Setup;
    }

    /// Opens Rules or About, remembering the screen to come back to.
    fn open_aside(&mut self, view: View) {
        if !matches!(self.view, View::Rules | View::About) {
            self.return_to = self.view;
        }
        self.view = view;
    }

    /// Writes the round to the store when it changed, or forgets it once
    /// there is no round any more.
    fn keep_round(&mut self, context: &mut Context) {
        let record = round::encode(self);
        if record == self.saved_round {
            return;
        }
        match &record {
            Some(text) => context.store().save(round::KEY, text.clone().into_bytes()),
            None => context.store().forget(round::KEY),
        }
        self.saved_round = record;
    }

    /// Picks up a round the store kept, if nothing has been started since.
    ///
    /// The store answers within moments of starting, but a quick tap can
    /// still begin a new round first, and that round is the one the table
    /// is now playing.
    fn resume_saved(&mut self, context: &mut Context, value: Option<Vec<u8>>) {
        let Some(bytes) = value else {
            return;
        };
        let Some(saved) = round::decode(&bytes) else {
            context.store().forget(round::KEY);
            self.saved_round = None;
            return;
        };
        if self.view != View::Home || self.entry.is_open() {
            return;
        }
        self.view = saved.view;
        self.party = saved.party;
        self.players = saved.players;
        self.question = saved.question;
        self.player = saved.player;
        self.scores = saved.scores;
        self.answers = saved.answers;
        self.round_questions = saved.questions;
        self.confirm = None;
        self.menu.close();
        self.note = None;
        self.page = 0;
        self.scorecard_saved = false;
        self.scorecard_refused = false;
        self.saved_round = round::encode(self);
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
            self.note = Some(Note::said(
                BannerLevel::Info,
                "Trivia packs are already updating.",
            ));
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
        } else if self.view == View::Reveal {
            let question = &self.round_questions[self.question % self.round_questions.len()];
            reveal_paged(self, question, context).map_or(1, |pages| pages.len())
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
    round_top(
        "pubquiz-pass",
        format!(
            "Question {} of {}",
            quiz.question + 1,
            quiz.round_questions.len()
        ),
        quiz.menu,
    )
    .splash(
        Some(Glyph::Person),
        format!("Pass to {next}"),
        format!("{previous} has answered. {next}, tap below when only you can see the screen."),
    )
    .game_actions(
        false,
        [("show-question", format!("Show {next}'s question"))],
    )
    .compose(|b| leave_prompt(quiz, b))
    .build()
}

/// The top of every screen of a round: where the round stands, Rules, and
/// the game menu with New game first.
///
/// Every screen of a round owns Back, because Back mid-round asks before
/// throwing the scores away rather than leaving the app.
fn round_top(name: &str, title: impl Into<String>, menu: Menu) -> ScreenBuilder {
    ScreenBuilder::new(name)
        .game_top_bar(title, menu, [(ABOUT, "About the questions")])
        .owns_back(true)
}

fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The results, with a refused scorecard said the fullest way that fits.
///
/// The retry belongs on the banner that says the scores were not saved. The
/// podium with four players has no room for that banner's action line, so
/// where it does not fit the banner names the top bar's Retry saving, and
/// where even the words do not fit the round line gives way.
fn podium_screen(quiz: &Quiz, context: &Context) -> Screen {
    if !quiz.scorecard_refused {
        return podium_shaped(quiz, Refusal::None);
    }
    let metrics = context.metrics();
    [
        Refusal::OnBanner,
        Refusal::InTopBar,
        Refusal::Compact,
        Refusal::BarOnly,
    ]
    .into_iter()
    .map(|shape| podium_shaped(quiz, shape))
    .find(|podium| {
        !podium
            .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
            .has_errors()
    })
    .unwrap_or_else(|| podium_shaped(quiz, Refusal::BarOnly))
}

/// How the podium says the scorecard was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Refusal {
    /// Nothing was refused.
    None,
    /// An error banner carrying Retry saving.
    OnBanner,
    /// An error banner naming the top bar's Retry saving.
    InTopBar,
    /// The same, without the round line under the bar.
    Compact,
    /// Only the bar's Retry saving, in place of Saved, where four players
    /// at the largest sizes leave no line for words.
    BarOnly,
}

fn podium_shaped(quiz: &Quiz, refusal: Refusal) -> Screen {
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
    let podium = ScreenBuilder::new("pubquiz-podium").top_bar("Results");
    // A refused scorecard says so with its one retry, so the screen has one
    // control for the job: on the banner where it fits, else in the bar.
    let podium = match refusal {
        Refusal::None => podium.top_bar_action(
            "save-scorecard",
            if quiz.scorecard_saved {
                "Saved"
            } else {
                "Save scores"
            },
        ),
        Refusal::OnBanner => podium
            .banner(
                BannerLevel::Error,
                "Scores could not be saved. Free some space on the reader.",
            )
            .banner_action("save-scorecard", "Retry saving"),
        Refusal::InTopBar | Refusal::Compact => podium
            .top_bar_action("save-scorecard", "Retry saving")
            .banner(
                BannerLevel::Error,
                "Scores not saved. Free some space, then tap Retry saving.",
            ),
        Refusal::BarOnly => podium.top_bar_action("save-scorecard", "Retry saving"),
    };
    let podium = if matches!(refusal, Refusal::Compact | Refusal::BarOnly) {
        podium
    } else {
        podium.secondary(format!(
            "Round {} · {}",
            quiz.rounds,
            questions_label(quiz.round_questions.len())
        ))
    };
    podium
        .heading(headline)
        .rows_with_trailing(rows)
        .primary_button("play-again", "Play again")
        .button("home", "Done")
        .build()
}

fn rounds_label(rounds: u16) -> String {
    if rounds == 1 {
        "1 round played".to_owned()
    } else {
        format!("{rounds} rounds played")
    }
}

/// Home, with its message drawn the fullest way that fits.
///
/// Home is full: a heading, two ways to play and the More rows. With a
/// message, and more so held sideways, the welcome heading gives way first,
/// then the line of counts, then the two ways to play share a line, then the
/// More heading and the rows' second lines, and last the banner's action
/// line. Without the action the message keeps its level and names the row
/// that asks again, which sits just below it.
fn home_screen(quiz: &Quiz, context: &Context) -> Screen {
    let metrics = context.metrics();
    let shapes: &[GiveWay] = if quiz.note.is_some() {
        &GiveWay::ALL
    } else {
        &[GiveWay::Nothing]
    };
    shapes
        .iter()
        .map(|shape| home_shaped(quiz, *shape))
        .find(|home| {
            !home
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        })
        .unwrap_or_else(|| home_shaped(quiz, shapes[shapes.len() - 1]))
}

/// How much of Home has given way to a message, in order: each step keeps
/// what the steps before it gave up.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum GiveWay {
    /// Everything is drawn.
    Nothing,
    /// The welcome heading.
    Heading,
    /// The line counting questions and rounds.
    Counts,
    /// The two ways to play share a line rather than stacking.
    Stack,
    /// The More heading, and the rows' second lines that are not news; the
    /// sync row still says when it is downloading.
    Rows,
    /// The banner's action line.
    Action,
}
impl GiveWay {
    const ALL: [Self; 6] = [
        Self::Nothing,
        Self::Heading,
        Self::Counts,
        Self::Stack,
        Self::Rows,
        Self::Action,
    ];
}

type HomeSlot = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>;

fn home_shaped(quiz: &Quiz, shape: GiveWay) -> Screen {
    let b = ScreenBuilder::new("pubquiz-home")
        .top_bar("Pub Quiz")
        .top_bar_action(game::RULES, game::RULES_LABEL);
    let b = if shape < GiveWay::Heading {
        b.heading("Ready when you are")
    } else {
        b
    };
    let b = if shape < GiveWay::Counts {
        b.secondary(format!(
            "{} · {}",
            questions_label(quiz.questions.len()),
            rounds_label(quiz.rounds)
        ))
    } else {
        b
    };
    let action = shape < GiveWay::Action;
    let b = match &quiz.note {
        None => b,
        Some(Note::Problem(problem)) if action => b.problem_banner_with(problem, "sync", None),
        // A problem's own words already say to try again, and the row that
        // does it is just below.
        Some(Note::Problem(problem)) => b.problem_banner(problem),
        Some(Note::Said {
            level,
            text,
            retry: true,
        }) if action => b.banner(*level, text).banner_action("sync", "Try again"),
        Some(Note::Said {
            level,
            text,
            retry: true,
        }) => b.banner(
            *level,
            format!("{text} Tap Get more questions to try again."),
        ),
        Some(Note::Said { level, text, .. }) => b.banner(*level, text),
    };
    let b = if shape >= GiveWay::Stack {
        let slots: [(kobo_sdk::SlotWidth, HomeSlot); 2] = [
            (
                kobo_sdk::SlotWidth::Fill,
                Box::new(|slot| slot.primary_button("party", "Play with friends")),
            ),
            (
                kobo_sdk::SlotWidth::Fill,
                Box::new(|slot| slot.button("solo", "Play solo")),
            ),
        ];
        b.band(kobo_sdk::BandAlign::Middle, slots)
    } else {
        b.primary_button("party", "Play with friends")
            .button("solo", "Play solo")
    };
    let lean = shape >= GiveWay::Rows;
    let b = if lean { b } else { b.section("More") };
    b.rows([
        (
            "sync",
            "Get more questions",
            if quiz.sync_task.is_some() {
                "Downloading from Open Trivia DB…".to_owned()
            } else if lean {
                String::new()
            } else {
                format!("{} · Needs Wi-Fi once", quiz.updated_label())
            },
            Glyph::Download,
        ),
        (
            ABOUT,
            "About the questions",
            if lean {
                String::new()
            } else {
                "Where they come from".to_owned()
            },
            Glyph::Book,
        ),
    ])
    .build()
}

/// Asks before a round is thrown away.
///
/// One press of Back mid-round used to drop every score on the table without
/// a word. Every screen of a round owns Back and routes it here instead, and
/// New game from the menu asks the same way.
fn leave_prompt(quiz: &Quiz, builder: ScreenBuilder) -> ScreenBuilder {
    match quiz.confirm {
        Some(Confirm::Leave) => builder.confirm(
            "Leave this round?",
            "The scores so far will be lost.",
            ("leave-round", "Leave round"),
            ("keep-playing", "Keep playing"),
        ),
        Some(Confirm::NewGame) => builder.confirm(
            "Start a new game?",
            "This round ends and its scores are lost.",
            (NEW_GAME_CONFIRMED, game::NEW_GAME_LABEL),
            ("keep-playing", "Keep playing"),
        ),
        None => builder,
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

/// The way back from Rules or About: to the round they were opened from, or
/// home.
fn aside_exit(quiz: &Quiz, home: (&'static str, &'static str)) -> (&'static str, &'static str) {
    if quiz.return_to.in_round() {
        ("resume", "Back to the round")
    } else {
        home
    }
}

fn about_screen(quiz: &Quiz) -> Screen {
    let (name, label) = aside_exit(quiz, ("home", "Back to packs"));
    ScreenBuilder::new("pubquiz-about")
        .top_bar("About the questions")
        .owns_back(true)
        .text("The bundled questions were written for Pub Quiz and ship with it.")
        .text("Get more questions adds a pack from Open Trivia DB, opentdb.com. Those questions are licensed CC BY-SA 4.0 and stay under that licence on this reader.")
        .rows([
            (
                "about-source".to_owned(),
                "Source".to_owned(),
                quiz.source_label(),
                Glyph::Download,
            ),
            (
                "about-updated".to_owned(),
                "Updated".to_owned(),
                quiz.updated_label(),
                Glyph::Clock,
            ),
        ])
        .button(name, label)
        .build()
}

fn rules_screen(quiz: &Quiz) -> Screen {
    let (name, label) = aside_exit(quiz, ("resume", "Done"));
    ScreenBuilder::new("pubquiz-rules")
        .top_bar(game::RULES_LABEL)
        .owns_back(true)
        .section("Solo")
        .text("Up to ten questions. A point for each right answer.")
        .section("With friends")
        .text("Two to four players pass the reader round, then see who was right.")
        .secondary("A round is saved if the reader sleeps.")
        .bottom_action(name, label)
        .build()
}

fn question_screen(quiz: &Quiz, context: &Context) -> Screen {
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    // The question in the display serif, so it reads as the card a quiz
    // master holds up rather than as one more line of interface.
    let compact = |menu| {
        round_top("pubquiz-question", question_title(quiz), menu)
            .secondary(round_position(quiz, question))
            .heading_at_level(2, &question.text)
            .rows(answer_rows(question))
    };
    // Measured without the leave prompt or an open menu: a dialogue over the
    // question is not the question failing to fit, and treating it as one
    // swapped the page behind it for the paginated layout. Measured with the
    // status band the reader draws above every screen, because a page that
    // fits only without it loses its last line on the panel.
    if compact(Menu::default())
        .build()
        .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
        .issues
        .is_empty()
    {
        return compact(quiz.menu)
            .compose(|b| leave_prompt(quiz, b))
            .build();
    }
    // Long questions stay complete, including answers that share a prefix.
    // The same measured prose pagination as the reader keeps every word reachable.
    let pages = context.paginate(&question_text(quiz), true);
    let page = quiz.page.min(pages.len().saturating_sub(1));
    let mut builder = round_top("pubquiz-question", question_title(quiz), quiz.menu);
    for paragraph in &pages[page] {
        builder = builder.text(paragraph);
    }
    builder
        .page_turns("previous-page", "next-page")
        .page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages.len()).unwrap_or(u16::MAX),
        )
        .game_actions(false, [("choose", "Answer")])
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
    round_top("pubquiz-choices", "Choose an answer", quiz.menu)
        .rows(pages[page].iter().map(|&index| {
            (
                choice(index),
                titles[index].as_str(),
                "",
                u16::try_from(index + 1).expect("four answers"),
            )
        }))
        .page_turns("previous-page", "next-page")
        .page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages.len()).unwrap_or(u16::MAX),
        )
        .game_actions(false, [("question", "Question")])
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

fn setup_prefix(quiz: &Quiz, note: bool) -> ScreenBuilder {
    let selected = DIFFICULTIES
        .iter()
        .position(|&(_, difficulty)| difficulty == quiz.setup_difficulty)
        .unwrap_or(0);
    let matching = quiz.pool(quiz.setup_category.as_deref()).len();
    let setup = ScreenBuilder::new("pubquiz-setup")
        .top_bar(if quiz.setup_party {
            "Pass-around round"
        } else {
            "Solo round"
        })
        .owns_back(true);
    // Too few questions for a round is said here, where the mix is chosen.
    // It may come right with another choice, so it asks for attention
    // rather than reporting a failure. The rows are paged under this
    // prefix, so the banner is measured with them.
    let setup = match &quiz.setup_note {
        Some(text) if note => setup.banner(BannerLevel::Attention, text),
        _ => setup,
    };
    setup
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

/// Whether Setup draws its note: wherever every page of rows still fits
/// under it, which is everywhere but a Clara held sideways at the largest
/// sizes. There the Category line's count, "1 question match", says it.
fn setup_note_shown(quiz: &Quiz, context: &Context) -> bool {
    quiz.setup_note.is_some() && {
        let metrics = context.metrics();
        let pages = setup_pages_with(quiz, context, true);
        (0..pages.len()).all(|page| {
            !setup_built(quiz, context, &pages, page, true)
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        })
    }
}

fn setup_pages(quiz: &Quiz, context: &Context) -> Vec<Vec<usize>> {
    setup_pages_with(quiz, context, setup_note_shown(quiz, context))
}

fn setup_pages_with(quiz: &Quiz, context: &Context, note: bool) -> Vec<Vec<usize>> {
    let rows = setup_rows(quiz, context);
    let measured: Vec<_> = rows
        .iter()
        .map(|(_, title, detail, _, _)| (title.as_str(), detail.as_str()))
        .collect();
    context.paginate_rows_under(
        &measured,
        true,
        kobo_sdk::Position::AtTheFoot,
        &setup_prefix(quiz, note).build(),
    )
}

fn setup_screen(quiz: &Quiz, context: &Context) -> Screen {
    let note = setup_note_shown(quiz, context);
    let pages = setup_pages_with(quiz, context, note);
    setup_built(quiz, context, &pages, quiz.setup_page, note)
}

fn setup_built(
    quiz: &Quiz,
    context: &Context,
    pages: &[Vec<usize>],
    page: usize,
    note: bool,
) -> Screen {
    let rows = setup_rows(quiz, context);
    let page = page.min(pages.len().saturating_sub(1));
    let mut builder = setup_prefix(quiz, note)
        .rows_with_trailing(pages[page].iter().map(|&index| rows[index].clone()));
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

/// One piece of the reveal, in the order the table reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Revealed {
    /// The category and the headline that says who got it.
    Verdict,
    /// The question, or as much of it as one page holds.
    Question(String),
    /// One answer, ticked when it was right, with who chose it.
    Answer(usize),
    /// The scores so far.
    Scores,
}

/// The headline that says who got the question.
fn verdict(quiz: &Quiz, question: &Question) -> String {
    let players = if quiz.party { quiz.players } else { 1 };
    let right: Vec<&str> = (0..players)
        .filter(|&i| quiz.answers[i] == Some(question.correct))
        .map(|i| quiz.name_for(i))
        .collect();
    match (quiz.party, right.len()) {
        (false, 0) => "Not this time".to_owned(),
        (false, _) => "Correct".to_owned(),
        (true, 0) => "Nobody got it".to_owned(),
        (true, n) if n == players => "Everyone got it".to_owned(),
        (true, _) => format!("{} got it", join_names(&right)),
    }
}

/// One answer row: ticked when it was right, and saying who chose it.
///
/// The arguing screen, where the table can see exactly how everyone went
/// wrong. Who chose it is in the row's summary rather than against its right
/// edge: that edge is for a short value, and four players' names there left
/// the answer itself a column a few pixels wide.
fn reveal_row(quiz: &Quiz, question: &Question, index: usize) -> (String, String, String, Glyph) {
    let players = if quiz.party { quiz.players } else { 1 };
    let chose: Vec<&str> = (0..players)
        .filter(|&i| quiz.answers[i] == Some(index))
        .map(|i| quiz.name_for(i))
        .collect();
    let right = index == question.correct;
    let who = if quiz.party {
        join_names(&chose)
    } else if chose.is_empty() {
        String::new()
    } else {
        "Your answer".to_owned()
    };
    let summary = match (right, who.is_empty()) {
        (true, true) => "Right answer".to_owned(),
        (true, false) => format!("Right answer · {who}"),
        (false, _) => who,
    };
    (
        format!("reveal-{index}"),
        question.answers[index].clone(),
        summary,
        if right { Glyph::Check } else { Glyph::Circle },
    )
}

/// The reveal's pieces, laid one after another on a page.
///
/// Answers next to each other are one list rather than a list each, so a page
/// holding all four draws them exactly as the single list it always was.
fn reveal_parts(
    quiz: &Quiz,
    question: &Question,
    parts: &[Revealed],
    mut builder: ScreenBuilder,
) -> ScreenBuilder {
    let mut index = 0;
    while index < parts.len() {
        match &parts[index] {
            Revealed::Verdict => {
                builder = builder
                    .secondary(&question.category)
                    .heading(verdict(quiz, question));
            }
            Revealed::Question(text) => builder = builder.text(text),
            Revealed::Answer(_) => {
                let mut rows = Vec::new();
                while let Some(Revealed::Answer(answer)) = parts.get(index) {
                    rows.push(reveal_row(quiz, question, *answer));
                    index += 1;
                }
                builder = builder.rows(rows);
                continue;
            }
            Revealed::Scores if quiz.party => {
                builder = builder.section("Scores").facts(
                    (0..quiz.players).map(|i| (quiz.name_for(i), points_label(quiz.scores[i]))),
                );
            }
            Revealed::Scores => {
                builder = builder.section_with_value(
                    "Score",
                    format!("{} of {}", quiz.scores[0], quiz.question + 1),
                );
            }
        }
        index += 1;
    }
    builder
}

/// One page of the reveal, without the leave prompt.
///
/// `pages` is `None` for the reveal that fits whole, which has no page
/// controls to draw. Continue is the bottom action on every page, so a table
/// that has seen enough never has to page to the end to move on. `menu` is
/// closed whenever the page is only being measured.
fn reveal_page(
    quiz: &Quiz,
    question: &Question,
    parts: &[Revealed],
    pages: Option<(usize, usize)>,
    menu: Menu,
) -> ScreenBuilder {
    let mut builder = reveal_parts(
        quiz,
        question,
        parts,
        round_top(
            "pubquiz-reveal",
            format!(
                "Question {} of {}",
                quiz.question + 1,
                quiz.round_questions.len()
            ),
            menu,
        ),
    );
    if let Some((page, count)) = pages {
        builder = builder
            .page_turns("previous-page", "next-page")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(count).unwrap_or(u16::MAX),
            );
    }
    builder.game_actions(
        false,
        [(
            "continue",
            if quiz.question + 1 == quiz.round_questions.len() {
                "See results"
            } else {
                "Next question"
            },
        )],
    )
}

/// Every piece of the reveal, whole.
fn reveal_all(question: &Question, text: Vec<String>) -> Vec<Revealed> {
    let mut parts = vec![Revealed::Verdict];
    parts.extend(text.into_iter().map(Revealed::Question));
    parts.extend((0..question.answers.len()).map(Revealed::Answer));
    parts.push(Revealed::Scores);
    parts
}

/// The reveal cut into pages that each fit this panel.
///
/// A synced question can run to 240 characters with four answers of 80, and
/// the reveal shows all of it beside who chose what and the scores: at the
/// largest text sizes on a small panel that ran off the foot with no way to
/// reach the rest. Each piece goes on the current page while the page still
/// measures clean with the runtime's own layout, and starts the next page when
/// it would not. The question itself is first broken by the same prose
/// pagination the question screen uses, so not even it is too tall for a page.
fn reveal_pages(quiz: &Quiz, question: &Question, context: &Context) -> Vec<Vec<Revealed>> {
    let fits = |parts: &[Revealed]| {
        reveal_page(quiz, question, parts, Some((0, 2)), Menu::default())
            .build()
            .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
            .issues
            .is_empty()
    };
    let text = context.paginate(&question.text, true).into_iter().flatten();
    let mut pages = Vec::new();
    let mut page = Vec::new();
    for part in reveal_all(question, text.collect()) {
        page.push(part);
        // A piece alone on a page stays there, because a page without it would
        // be no closer to fitting; the layout tests are what keep that from
        // ever being needed.
        if page.len() > 1 && !fits(&page) {
            let part = page.pop().expect("a piece was just placed");
            pages.push(std::mem::take(&mut page));
            page.push(part);
        }
    }
    pages.push(page);
    pages
}

/// The reveal's pages, or `None` when it fits whole on one screen.
///
/// Measured without the leave prompt, for the same reason as the question: a
/// dialogue over the reveal is not the reveal failing to fit.
fn reveal_paged(quiz: &Quiz, question: &Question, context: &Context) -> Option<Vec<Vec<Revealed>>> {
    let whole = reveal_all(question, vec![question.text.clone()]);
    let fits = reveal_page(quiz, question, &whole, None, Menu::default())
        .build()
        .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
        .issues
        .is_empty();
    (!fits).then(|| reveal_pages(quiz, question, context))
}

fn reveal_screen(quiz: &Quiz, question: &Question, context: &Context) -> Screen {
    let Some(pages) = reveal_paged(quiz, question, context) else {
        let whole = reveal_all(question, vec![question.text.clone()]);
        return reveal_page(quiz, question, &whole, None, quiz.menu)
            .compose(|b| leave_prompt(quiz, b))
            .build();
    };
    let page = quiz.page.min(pages.len().saturating_sub(1));
    reveal_page(
        quiz,
        question,
        &pages[page],
        Some((page, pages.len())),
        quiz.menu,
    )
    .compose(|b| leave_prompt(quiz, b))
    .build()
}

fn screen_with(quiz: &Quiz, context: &Context) -> Screen {
    if quiz.entry.is_open() {
        return ScreenBuilder::new("pubquiz-rename")
            .top_bar("Rename player")
            .owns_back(true)
            // The rule rides with the field, because the runtime's keyboard
            // covers this screen and shows only the prompt and the hint.
            .text_entry_with_hint(
                &quiz.entry,
                "Player name",
                "Letters and digits, twelve characters or fewer.",
                "Done",
            )
            .build();
    }
    let question = &quiz.round_questions[quiz.question % quiz.round_questions.len()];
    match quiz.view {
        View::Setup => setup_screen(quiz, context),
        View::Players => players_screen(quiz),
        View::Home => home_screen(quiz, context),
        View::Question => question_screen(quiz, context),
        View::Choices => choices_screen(quiz, context),
        View::Pass => pass_screen(quiz),
        View::Reveal => reveal_screen(quiz, question, context),
        View::Podium => podium_screen(quiz, context),
        View::Rules => rules_screen(quiz),
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
        context.store().load(round::KEY);
        self.show(context);
    }
    /// Asks the store for the round again when there is none on screen.
    ///
    /// Usually the process slept with everything in memory and the round is
    /// already showing. When it did not, the store has the copy written after
    /// the last answer.
    fn on_resume(&mut self, context: &mut Context) {
        if self.view == View::Home && !self.entry.is_open() {
            context.store().load(round::KEY);
        }
        self.show(context);
    }
    fn on_background(&mut self, context: &mut Context) {
        self.keep_round(context);
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
                        self.questions = with_bundled(questions);
                        self.packs = 1;
                        if self.pack_origin.is_none() {
                            self.pack_origin = Some("Open Trivia DB".to_owned());
                        }
                    } else {
                        context.store().forget(PACK);
                        self.note = Some(Note::said(
                            BannerLevel::Info,
                            "Saved questions were damaged; the built-in set is ready.",
                        ));
                    }
                }
            } else if key == round::KEY {
                self.resume_saved(context, value);
            }
            self.show(context);
        }
    }
    /// A refused write is said rather than dropped. The scorecard is
    /// answered on the podium it was saved from; anything else on Home.
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if !matches!(result, StoreResult::Denied(_)) {
            self.on_store(context, result);
            return;
        }
        if key == SCORECARD {
            self.scorecard_saved = false;
            self.scorecard_refused = true;
        } else {
            let lost = match key {
                PACK => "New questions could not be kept for offline play.",
                k if k == round::KEY => "This round could not be saved if the reader sleeps.",
                LICENSE => "The question licence could not be written.",
                _ => "Player names and rounds played could not be saved.",
            };
            self.note = Some(Note::said(
                BannerLevel::Error,
                format!("{lost} Free some space on the reader."),
            ));
        }
        self.show(context);
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.sync_task != Some(task) {
            return;
        }
        self.sync_task = None;
        // A failure is told in Open Trivia DB's terms, at the level its
        // cause deserves, with Join Wi-Fi or Try again on the banner.
        if let Some(mut problem) = SOURCE.problem(&outcome) {
            // The address is Pub Quiz's own, so there is no server address
            // for the owner to check; what is left to do is wait.
            if problem.cause == kobo_sdk::failure::Cause::NotFound {
                "Open Trivia DB sent no questions. Try again later."
                    .clone_into(&mut problem.message);
            }
            self.note = Some(Note::Problem(problem));
            self.show(context);
            return;
        }
        self.note = Some(match outcome {
            TaskOutcome::Completed(bytes) => {
                if let Some(questions) = parse_pack(&bytes) {
                    let fresh = questions.len();
                    self.questions = with_bundled(questions);
                    self.pack_synced = true;
                    self.packs = 1;
                    self.pack_origin = Some("Open Trivia DB".to_owned());
                    self.pack_updated_min = now_minutes();
                    context.store().save(PACK, bytes);
                    self.save(context);
                    Note::said(
                        BannerLevel::Success,
                        format!("{fresh} fresh questions saved for offline play."),
                    )
                } else {
                    Note::Said {
                        level: BannerLevel::Error,
                        text: "Open Trivia DB returned no usable question set.".to_owned(),
                        retry: true,
                    }
                }
            }
            // Nothing failed, the download was stopped; asking again is the
            // whole of the fix.
            TaskOutcome::Failed(_) | TaskOutcome::Cancelled => Note::Said {
                level: BannerLevel::Attention,
                text: "The download stopped before it finished.".to_owned(),
                retry: true,
            },
        });
        self.show(context);
    }
    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        let paged = matches!(
            self.view,
            View::Setup | View::Question | View::Choices | View::Reveal
        );
        if paged && !self.entry.is_open() {
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
        let in_round = self.view.in_round();
        // The game controls are read first on a round's screens and at home,
        // where Rules also sits in the top bar; a confirmation over the round
        // is the only thing that answers while it is up.
        let tap = if self.confirm.is_none() && (in_round || self.view == View::Home) {
            self.menu.read(action)
        } else {
            None
        };
        if let Some(asking) = self.confirm {
            if action == action_id("leave-round") && asking == Confirm::Leave {
                self.confirm = None;
                self.view = View::Home;
            } else if action == action_id(NEW_GAME_CONFIRMED) && asking == Confirm::NewGame {
                self.new_game();
            } else if action == action_id("keep-playing") || action == ActionId::BACK {
                self.confirm = None;
            }
        } else if let Some(tap) = tap {
            match tap {
                // A quiz has no undo, so the bar never offers one.
                Tap::Menu | Tap::Undo => {}
                Tap::Rules => self.open_aside(View::Rules),
                Tap::NewGame if in_round => self.confirm = Some(Confirm::NewGame),
                Tap::NewGame => self.new_game(),
            }
        } else if action == action_id(ABOUT) && (in_round || self.view == View::Home) {
            self.open_aside(View::About);
        } else if matches!(self.view, View::Rules | View::About)
            && (action == ActionId::BACK || action == action_id("resume"))
        {
            self.view = self.return_to;
        } else if action == ActionId::BACK && in_round {
            self.confirm = Some(Confirm::Leave);
        } else if action == ActionId::BACK && self.view == View::Players {
            self.view = View::Setup;
        } else if action == action_id("choose") && self.view == View::Question {
            self.view = View::Choices;
            self.page = 0;
        } else if action == action_id("question") && self.view == View::Choices {
            self.view = View::Question;
            self.page = 0;
        } else if matches!(self.view, View::Question | View::Choices | View::Reveal)
            && action == action_id("next-page")
        {
            self.page = self
                .page
                .saturating_add(1)
                .min(self.page_count(context).saturating_sub(1));
        } else if matches!(self.view, View::Question | View::Choices | View::Reveal)
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
                // Opened with the name in it, so a typo is mended rather
                // than the whole name typed again.
                self.entry.open_with(self.names[index].clone());
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
            self.scorecard_refused = false;
        } else if action == action_id("play-again") && self.view == View::Podium {
            self.begin(self.party);
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
        self.keep_round(context);
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
    fn every_bundled_category_is_a_full_round_and_the_rules_are_short() {
        let quiz = Quiz::default();
        for category in setup_categories(&quiz) {
            assert!(
                quiz.pool(Some(&category)).len() >= ROUND_LENGTH,
                "{category}"
            );
        }
        let quiz = Quiz {
            view: View::Rules,
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
        assert_eq!(quiz.view, View::Setup);
        assert!(quiz.setup_note.is_some());

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
        // A synced pack often holds only a few questions in a category.
        let mut quiz = Quiz {
            questions: with_bundled(
                (0..6)
                    .map(|index| {
                        question(
                            "Mythology",
                            Difficulty::Medium,
                            &format!("Small pack question {index}?"),
                            ["A", "B", "C", "D"],
                            0,
                        )
                    })
                    .collect(),
            ),
            setup_category: Some("Mythology".to_owned()),
            ..Quiz::default()
        };
        let pool = quiz.pool(Some("Mythology"));
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
        let shown = format!("{:?}", reveal_screen(&quiz, &question, &Context::default()));
        assert!(shown.contains("Ada got it"));
        assert!(shown.contains("Right answer"));
        assert!(shown.contains("Bert and Cleo"));
        assert!(shown.contains(&format!("Question 1 of {}", quiz.round_questions.len())));

        quiz.answers = [Some(wrong); 4];
        let shown = format!("{:?}", reveal_screen(&quiz, &question, &Context::default()));
        assert!(shown.contains("Nobody got it"));

        let mut solo = Quiz::default();
        solo.begin(false);
        let question = solo.round_questions[0].clone();
        solo.answers[0] = Some(question.correct);
        let shown = format!("{:?}", reveal_screen(&solo, &question, &Context::default()));
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
        let shown = format!("{:?}", reveal_screen(&quiz, &question, &Context::default()));
        assert!(shown.contains("See results"));
        let mut full = Quiz::default();
        full.begin(false);
        let first = full.round_questions[0].clone();
        full.answers[0] = Some(first.correct);
        let shown = format!("{:?}", reveal_screen(&full, &first, &Context::default()));
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
    use kobo_ui::{Orientation, TextScale};

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
        runner.type_text("Sam");
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
        assert_eq!(runner.app().confirm, Some(Confirm::Leave));
        assert_eq!(runner.app().view, View::Question);
        assert!(format!("{:?}", screen(runner.app())).contains("Leave this round?"));
        runner.action(action_id("keep-playing"));
        assert_eq!(runner.app().confirm, None);
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
        let note = runner.app().note.clone().expect("a note");
        assert_eq!(note.level(), BannerLevel::Attention);
        assert_eq!(
            note,
            Note::Problem(SOURCE.explain(kobo_sdk::TaskError::Offline))
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

    /// A round whose first question is as long as a synced pack allows: 240
    /// characters, and four answers of 80 that share everything but their
    /// endings.
    fn long_question_round(party: bool) -> Quiz {
        let mut quiz = Quiz::default();
        quiz.begin(party);
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
        quiz
    }

    /// Pages through the reveal as the reader would, and checks that every
    /// page fits, that Continue is on every one of them, and that the verdict,
    /// the whole question, every answer and the scores are each on some page.
    fn assert_the_reveal_pages_reach_everything(quiz: Quiz, metrics: DisplayMetrics) {
        let question = quiz.round_questions[0].clone();
        let mut runner = AppRunner::with_metrics(quiz, metrics);
        runner.app_mut().view = View::Reveal;
        runner.app_mut().page = 0;
        let context = runner.context();
        let pages = reveal_paged(runner.app(), &question, &context)
            .unwrap_or_else(|| vec![reveal_all(&question, vec![question.text.clone()])]);
        let mut answers = [false; 4];
        for page in 0..pages.len() {
            assert_eq!(runner.app().page, page, "{metrics:?}");
            let screen = reveal_screen(runner.app(), &question, &context);
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{metrics:?} page {page}: {:?}",
                diagnostics.issues
            );
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            assert!(
                layout.rect_of_action(action_id("continue")).is_some(),
                "{metrics:?} page {page} has no way on"
            );
            for (index, found) in answers.iter_mut().enumerate() {
                *found |= layout
                    .rect_of_action(action_id(&format!("reveal-{index}")))
                    .is_some();
            }
            runner.action(action_id("next-page"));
        }
        // Paging past the end stays on the last page rather than wrapping.
        assert_eq!(runner.app().page, pages.len() - 1, "{metrics:?}");
        assert!(answers.into_iter().all(|found| found), "{metrics:?}");
        let parts = pages.concat();
        assert!(parts.contains(&Revealed::Verdict), "{metrics:?}");
        assert!(parts.contains(&Revealed::Scores), "{metrics:?}");
        let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let asked = parts
            .iter()
            .filter_map(|part| match part {
                Revealed::Question(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(words(&asked), words(&question.text), "{metrics:?}");
    }

    /// The round an owner meets first: a short question that two players both
    /// got right. Its verdict, question and the row naming both of them were
    /// measured without the status band, so at the largest size the page
    /// fitted in the app and the reader refused it.
    #[test]
    fn a_short_question_everyone_got_fits_every_text_size_on_a_clara_held_either_way() {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..kobo_ui::CLARA_BW_METRICS
                }
                .oriented(orientation);
                let table = || {
                    let mut table = Quiz {
                        players: 2,
                        ..Quiz::default()
                    };
                    table.begin(true);
                    table.round_questions[0] = bank::bundled()[0].clone();
                    table.answers = [Some(table.round_questions[0].correct); 4];
                    table
                };
                assert_the_reveal_pages_reach_everything(table(), metrics);
                let mut solo = Quiz::default();
                solo.begin(false);
                solo.round_questions[0] = bank::bundled()[0].clone();
                solo.answers[0] = Some(solo.round_questions[0].correct + 1);
                assert_the_reveal_pages_reach_everything(solo, metrics);
                let context = AppRunner::with_metrics(Quiz::default(), metrics).context();
                let table = table();
                for screen in [
                    question_screen(&table, &context),
                    choices_screen(&table, &context),
                ] {
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                }
            }
        }
    }

    #[test]
    fn long_questions_keep_every_answer_reachable_at_large_text_sizes() {
        let mut quiz = long_question_round(false);
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
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
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
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                    for (index, found) in seen.iter_mut().enumerate() {
                        *found |= layout.rect_of_action(action_id(&choice(index))).is_some();
                    }
                }
                assert!(seen.into_iter().all(|found| found));

                // The reveal adds who chose what and the scores to all of it.
                // A solo round, and a full table of the longest names a player
                // can have, every one of them on the same wrong answer so its
                // row carries the longest list of who chose it.
                let mut solo = long_question_round(false);
                solo.answers[0] = Some(solo.round_questions[0].correct);
                assert_the_reveal_pages_reach_everything(solo, metrics);
                let mut table = long_question_round(true);
                table.names =
                    ["Wilhelmina A", "Bartholomew", "Maximiliana", "Christopher"].map(String::from);
                let wrong = (table.round_questions[0].correct + 1) % 4;
                table.answers = [Some(wrong); 4];
                table.scores = [9; 4];
                table.player = 3;
                assert_the_reveal_pages_reach_everything(table, metrics);
            }
        }
    }
}

#[cfg(test)]
mod help_layout_tests {
    use super::*;
    #[test]
    fn the_rules_fit_every_text_size_and_pose_with_their_way_back() {
        let from_home = Quiz {
            view: View::Rules,
            ..Quiz::default()
        };
        let from_round = Quiz {
            view: View::Rules,
            return_to: View::Reveal,
            ..Quiz::default()
        };
        assert!(format!("{:?}", screen(&from_round)).contains("Back to the round"));
        for quiz in [&from_home, &from_round] {
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
                    // A runner installs the face the runtime measures with, so
                    // the screen is built after one. The face is installed once
                    // per process, at the density of whichever panel installs it
                    // first, so the 212 ppi panel here is measured with 300 ppi
                    // type and is stricter than a reader: the copy is short
                    // enough to fit even so.
                    let _runner = kobo_sdk::AppRunner::with_metrics(Quiz::default(), metrics);
                    let screen = screen(quiz);
                    let chrome = kobo_ui::Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    assert!(screen
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id("resume"))
                        .is_some());
                }
            }
        }
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::*;
    use kobo_sdk::AppRunner;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn solo_round() -> AppRunner<Quiz> {
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        assert_eq!(runner.app().view, View::Question);
        runner
    }

    fn labels(screen: &Screen) -> Vec<String> {
        screen
            .top_bar
            .as_ref()
            .map(|bar| bar.actions.iter().map(|a| a.label.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn every_screen_of_a_round_carries_rules_and_the_menu() {
        let mut runner = AppRunner::new(Quiz {
            players: 2,
            ..Quiz::default()
        });
        runner.start();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("start"));
        for view in [View::Question, View::Pass, View::Question, View::Reveal] {
            assert_eq!(runner.app().view, view);
            let shown = screen(runner.app());
            assert_eq!(labels(&shown), ["Rules", "More"], "{view:?}");
            assert!(shown
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
            match view {
                View::Pass => runner.action(action_id("show-question")),
                _ => runner.action(action_id("answer-0")),
            };
        }
        runner.app_mut().view = View::Choices;
        assert_eq!(labels(&screen(runner.app())), ["Rules", "More"]);
    }

    #[test]
    fn the_menu_opens_with_new_game_first_and_closes_on_back() {
        let mut runner = solo_round();
        runner.action(action_id(game::MORE));
        let shown = format!("{:?}", screen(runner.app()));
        let new_game = shown.find("New game").expect("New game is offered");
        let about = shown.find("About the questions").expect("About is offered");
        assert!(new_game < about);
        runner.action(ActionId::BACK);
        assert!(!runner.app().menu.is_open());
        assert_eq!(runner.app().view, View::Question);
        assert_eq!(
            runner.app().confirm,
            None,
            "closing the menu is not leaving"
        );
    }

    #[test]
    fn new_game_mid_round_asks_first_then_goes_back_to_the_choices() {
        let mut runner = solo_round();
        runner.action(action_id("answer-0"));
        runner.action(action_id(game::MORE));
        runner.action(action_id(game::NEW_GAME));
        assert_eq!(runner.app().confirm, Some(Confirm::NewGame));
        assert!(format!("{:?}", screen(runner.app())).contains("Start a new game?"));
        runner.action(action_id("keep-playing"));
        assert_eq!(
            runner.app().view,
            View::Reveal,
            "keeping the round keeps it"
        );

        runner.action(action_id(game::MORE));
        runner.action(action_id(game::NEW_GAME));
        runner.action(action_id(NEW_GAME_CONFIRMED));
        assert_eq!(runner.app().view, View::Setup);
        assert!(!runner.app().setup_party, "a solo round starts a solo game");
        assert!(round::encode(runner.app()).is_none());
    }

    #[test]
    fn rules_and_about_from_a_round_go_back_to_the_same_screen() {
        let mut runner = solo_round();
        runner.action(action_id("answer-1"));
        let before = (runner.app().question, runner.app().scores);
        runner.action(action_id(game::RULES));
        assert_eq!(runner.app().view, View::Rules);
        runner.action(action_id("resume"));
        assert_eq!(runner.app().view, View::Reveal);

        runner.action(action_id(game::MORE));
        runner.action(action_id(ABOUT));
        assert_eq!(runner.app().view, View::About);
        assert!(format!("{:?}", screen(runner.app())).contains("Back to the round"));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Reveal);
        assert_eq!((runner.app().question, runner.app().scores), before);
    }

    #[test]
    fn home_offers_rules_in_the_top_bar_and_no_separate_how_to_play() {
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        let home = screen(runner.app());
        assert_eq!(labels(&home), ["Rules"]);
        let layout = home.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("how-to-play")).is_none());
        assert!(!format!("{home:?}").contains("How to play"));
        runner.action(action_id(game::RULES));
        assert_eq!(runner.app().view, View::Rules);
        assert!(format!("{:?}", screen(runner.app())).contains("pass the reader round"));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
    }

    #[test]
    fn a_synced_pack_adds_to_the_bundled_questions() {
        let item = r#"{"category":"Mythology","question":"Synced question?","correct_answer":"Right","incorrect_answers":["W1","W2","W3"]}"#;
        let body = format!(
            r#"{{"response_code":0,"results":[{}]}}"#,
            [item; 10].join(",")
        );
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("sync"));
        let task = runner.app().sync_task.expect("sync started");
        runner.task_outcome(task, TaskOutcome::Completed(body.into_bytes()));
        assert_eq!(runner.app().questions.len(), bank::bundled().len() + 10);
        assert_eq!(
            runner.app().note,
            Some(Note::said(
                BannerLevel::Success,
                "10 fresh questions saved for offline play."
            ))
        );
        assert_eq!(runner.app().source_label(), "Bundled and Open Trivia DB");
    }
}

#[cfg(test)]
mod setup_tests;

#[cfg(test)]
mod p16_tests {
    use super::*;
    use kobo_sdk::{AppRunner, Chrome, Command, DisplayMetrics, StoreError, TaskError};
    use kobo_ui::{Node, Orientation, TextScale};

    fn banners(screen: &Screen) -> Vec<(BannerLevel, String, Option<String>)> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Banner {
                    level,
                    text,
                    action,
                    ..
                } => Some((
                    *level,
                    text.clone(),
                    action.as_ref().map(|action| action.label.clone()),
                )),
                _ => None,
            })
            .collect()
    }

    /// A started quiz whose first four store requests are answered, so the
    /// next refusal answers the request a test makes.
    fn started() -> AppRunner<Quiz> {
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        for key in [STATE, PACK] {
            runner.store_result(StoreResult::Loaded {
                key: key.into(),
                value: None,
            });
        }
        runner.store_result(StoreResult::Saved {
            key: LICENSE.into(),
        });
        runner.store_result(StoreResult::Loaded {
            key: round::KEY.into(),
            value: None,
        });
        runner
    }

    fn spawned(commands: &[Command]) -> TaskId {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .expect("a task")
    }

    /// A sync answered with `outcome`, after the quiet second chance a
    /// retrying fetch gives a failure.
    fn synced(outcome: &TaskOutcome) -> AppRunner<Quiz> {
        let mut runner = started();
        runner.action(action_id("sync"));
        let mut task = runner.app().sync_task.expect("sync started");
        for _ in 0..3 {
            let commands = runner.task_outcome(task, outcome.clone());
            if runner.app().sync_task.is_none() {
                break;
            }
            let nap = spawned(&commands);
            task = spawned(&runner.task_outcome(nap, TaskOutcome::Completed(Vec::new())));
        }
        assert!(runner.app().sync_task.is_none(), "the sync never settled");
        runner
    }

    /// Every message Home can carry.
    fn notes() -> Vec<Note> {
        vec![
            Note::said(
                BannerLevel::Success,
                "50 fresh questions saved for offline play.",
            ),
            Note::Problem(SOURCE.explain(TaskError::Offline)),
            Note::Problem(SOURCE.explain(TaskError::Unreachable)),
            Note::Problem(SOURCE.explain(TaskError::TimedOut)),
            Note::Problem(SOURCE.explain(TaskError::RateLimited(600))),
            Note::Problem(Problem {
                message: "Open Trivia DB sent no questions. Try again later.".to_owned(),
                ..SOURCE.explain(TaskError::NotFound)
            }),
            Note::Said {
                level: BannerLevel::Error,
                text: "Open Trivia DB returned no usable question set.".to_owned(),
                retry: true,
            },
            Note::Said {
                level: BannerLevel::Attention,
                text: "The download stopped before it finished.".to_owned(),
                retry: true,
            },
            Note::said(BannerLevel::Info, "Trivia packs are already updating."),
            Note::said(
                BannerLevel::Info,
                "Saved questions were damaged; the built-in set is ready.",
            ),
            Note::said(
                BannerLevel::Error,
                "Player names and rounds played could not be saved. Free some space on the reader.",
            ),
            Note::said(
                BannerLevel::Error,
                "This round could not be saved if the reader sleeps. Free some space on the reader.",
            ),
        ]
    }

    /// A fresh pack is a success, a pack that cannot be used is an error
    /// with Try again on it, and a failed download is told in Open Trivia
    /// DB's terms with the control that may fix it, so a failure no longer
    /// looks like a success with its retry under another name further down.
    #[test]
    fn sync_notes_carry_a_level_and_the_retry_sits_on_the_banner() {
        let item = r#"{"category":"Mythology","question":"Synced question?","correct_answer":"Right","incorrect_answers":["W1","W2","W3"]}"#;
        let body = format!(
            r#"{{"response_code":0,"results":[{}]}}"#,
            [item; 10].join(",")
        );
        let fresh = synced(&TaskOutcome::Completed(body.into_bytes()));
        assert_eq!(
            banners(&screen(fresh.app())),
            [(
                BannerLevel::Success,
                "10 fresh questions saved for offline play.".to_owned(),
                None
            )]
        );

        let unusable = synced(&TaskOutcome::Completed(b"not a pack".to_vec()));
        assert_eq!(
            banners(&screen(unusable.app())),
            [(
                BannerLevel::Error,
                "Open Trivia DB returned no usable question set.".to_owned(),
                Some("Try again".to_owned())
            )]
        );

        let mut away = synced(&TaskOutcome::Failed(TaskError::TimedOut));
        let drawn = banners(&screen(away.app()));
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].0, BannerLevel::Attention);
        assert!(drawn[0].1.starts_with("Open Trivia DB"), "{drawn:?}");
        assert_eq!(drawn[0].2.as_deref(), Some("Try again"));
        // The banner's Try again asks again.
        let commands = away.action(action_id("sync"));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
    }

    /// Home holds a heading, two ways to play, a banner and the More rows,
    /// so every message it can carry, with its action line, is measured on
    /// a Clara at every size, held either way, with the sync row both idle
    /// and downloading.
    #[test]
    fn home_fits_every_note_at_every_size_either_way_up() {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..kobo_ui::CLARA_BW_METRICS
                }
                .oriented(orientation);
                let runner = AppRunner::with_metrics(Quiz::default(), metrics);
                for note in notes() {
                    for downloading in [false, true] {
                        let mut quiz = Quiz {
                            note: Some(note.clone()),
                            ..Quiz::default()
                        };
                        if downloading {
                            quiz.sync_task = Some(TaskId(1));
                        }
                        let home = screen_with(&quiz, &runner.context());
                        let chrome = Chrome::measuring(true);
                        let issues = home.diagnostics(&metrics, &chrome).issues;
                        assert!(
                            issues.is_empty(),
                            "{metrics:?} {:?}: {issues:?}",
                            note.text()
                        );
                        let layout = home.layout_with(&metrics, &chrome);
                        // Upright at the default size nothing gives way: the
                        // welcome heading and the banner's action are there.
                        if orientation == Orientation::Portrait && text_scale == TextScale::Default
                        {
                            let drawn = format!("{home:?}");
                            assert!(drawn.contains("Ready when you are"), "{:?}", note.text());
                            let carries = matches!(&note, Note::Said { retry: true, .. })
                                || matches!(&note, Note::Problem(problem) if problem.retryable);
                            if carries {
                                assert!(
                                    banners(&home).iter().any(|banner| banner.2.is_some()),
                                    "{:?}",
                                    note.text()
                                );
                            }
                        }
                        for name in ["party", "solo", "sync", ABOUT] {
                            assert!(
                                layout.rect_of_action(action_id(name)).is_some(),
                                "{metrics:?} {:?}: {name} is off the panel",
                                note.text()
                            );
                        }
                    }
                }
            }
        }
    }

    /// The rename field carries its rule as the runtime keyboard's hint,
    /// since the keyboard covers the app's own screen, and it opens with the
    /// name already in it so a typo is mended rather than retyped.
    #[test]
    fn renaming_shows_the_rule_on_the_keyboard_and_starts_from_the_name() {
        let mut runner = started();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        assert_eq!(runner.app().view, View::Players);
        let commands = runner.action(action_id("rename-1"));
        assert!(runner.app().entry.is_open());
        assert_eq!(runner.app().entry.text(), "Bert");
        match kobo_sdk::keyboard::testing::asked(&commands) {
            Some(kobo_sdk::DeviceRequest::TypeText { hint, initial, .. }) => {
                assert_eq!(hint, "Letters and digits, twelve characters or fewer.");
                assert_eq!(initial, "Bert");
            }
            other => panic!("no field was asked for: {other:?}"),
        }
        let rename = screen_with(runner.app(), &runner.context());
        let secondary: Vec<&str> = rename
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Secondary { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            secondary,
            ["Letters and digits, twelve characters or fewer."],
            "the rule is drawn once, as the field's hint"
        );
    }

    /// A refused write is said. The scorecard is answered on the podium,
    /// where Retry saving on the banner replaces the top bar's Save scores
    /// and "Saved" is never claimed; anything else is an error on Home.
    #[test]
    fn a_refused_write_is_said_and_a_refused_scorecard_can_be_saved_again() {
        let mut runner = started();
        runner.app_mut().view = View::Podium;
        runner.action(action_id("save-scorecard"));
        assert!(runner.app().scorecard_saved);
        runner.store_result(StoreResult::Denied(StoreError::NoRoom));
        let podium = screen_with(runner.app(), &runner.context());
        assert!(!runner.app().scorecard_saved);
        let drawn = banners(&podium);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].0, BannerLevel::Error);
        let in_bar: Vec<&str> = podium
            .top_bar
            .as_ref()
            .map(|bar| bar.actions.iter().map(|a| a.label.as_str()).collect())
            .unwrap_or_default();
        // One Retry saving: on the banner, or in the bar where the banner's
        // action line does not fit; never a claim that it is saved.
        let retries = usize::from(drawn[0].2.as_deref() == Some("Retry saving"))
            + in_bar
                .iter()
                .filter(|label| **label == "Retry saving")
                .count();
        assert_eq!(retries, 1, "{drawn:?} {in_bar:?}");
        assert!(!in_bar.contains(&"Saved"));
        let commands = runner.action(action_id("save-scorecard"));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == SCORECARD
        )));
        assert!(banners(&screen(runner.app())).is_empty());

        let mut runner = started();
        runner.app_mut().view = View::Players;
        runner.action(action_id("count-3"));
        runner.store_result(StoreResult::Denied(StoreError::NoRoom));
        runner.app_mut().view = View::Home;
        let home = banners(&screen(runner.app()));
        assert_eq!(home.len(), 1);
        assert_eq!(home[0].0, BannerLevel::Error);
        assert!(home[0].1.contains("Free some space"), "{home:?}");
    }

    /// The refused scorecard's banner keeps the podium on the panel with
    /// four players, at every size, held either way.
    #[test]
    fn a_refused_scorecard_fits_the_podium_at_every_size_either_way_up() {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..kobo_ui::CLARA_BW_METRICS
                }
                .oriented(orientation);
                let runner = AppRunner::with_metrics(Quiz::default(), metrics);
                let mut quiz = Quiz::default();
                quiz.begin(true);
                quiz.view = View::Podium;
                quiz.scores = [3, 7, 7, 1];
                // Four players and a shared win already fill the podium at
                // the largest sizes; what this guards is that a refusal never
                // pushes a podium that fits off the panel.
                quiz.scorecard_refused = false;
                let plain = screen_with(&quiz, &runner.context());
                if plain
                    .diagnostics(&metrics, &Chrome::measuring(true))
                    .has_errors()
                {
                    continue;
                }
                for refused in [false, true] {
                    quiz.scorecard_refused = refused;
                    let podium = screen_with(&quiz, &runner.context());
                    let chrome = Chrome::measuring(true);
                    let issues = podium.diagnostics(&metrics, &chrome).issues;
                    assert!(issues.is_empty(), "{metrics:?} {refused}: {issues:?}");
                    if refused
                        && orientation == Orientation::Portrait
                        && text_scale == TextScale::Default
                    {
                        assert_eq!(banners(&podium).len(), 1, "the refusal is said");
                    }
                    let layout = podium.layout_with(&metrics, &chrome);
                    for name in ["save-scorecard", "play-again", "home"] {
                        assert!(
                            layout.rect_of_action(action_id(name)).is_some(),
                            "{metrics:?} {refused}: {name} is off the panel"
                        );
                    }
                }
            }
        }
    }

    /// Too few questions for a round is said on Setup, where the category
    /// and difficulty are chosen, as something to attend to, and the rows
    /// under it are paged with the banner measured, at every size either
    /// way up.
    #[test]
    fn too_few_questions_is_said_on_setup_and_the_rows_still_fit() {
        let mut runner = started();
        runner.action(action_id("solo"));
        runner.app_mut().setup_category = Some("No such category".to_owned());
        runner.action(action_id("continue-setup"));
        assert_eq!(runner.app().view, View::Setup);
        let drawn = banners(&screen_with(runner.app(), &runner.context()));
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].0, BannerLevel::Attention);
        assert!(runner.app().note.is_none());
        runner.action(action_id("cat-any"));
        assert!(banners(&screen_with(runner.app(), &runner.context())).is_empty());

        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            for text_scale in TextScale::STEPS {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..kobo_ui::CLARA_BW_METRICS
                }
                .oriented(orientation);
                let runner = AppRunner::with_metrics(Quiz::default(), metrics);
                let context = runner.context();
                // A category and difficulty with one or two questions, for
                // the longer of the two messages, with every category row.
                let mut quiz = Quiz {
                    view: View::Setup,
                    ..Quiz::default()
                };
                let sparse = setup_categories(&quiz).into_iter().find_map(|category| {
                    [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard]
                        .into_iter()
                        .find(|difficulty| {
                            let count = quiz
                                .questions
                                .iter()
                                .filter(|q| q.category == category && q.difficulty == *difficulty)
                                .count();
                            (1..MIN_ROUND).contains(&count)
                        })
                        .map(|difficulty| (category, difficulty))
                });
                let (category, difficulty) = sparse.unwrap_or_else(|| {
                    quiz.questions.truncate(2);
                    (
                        quiz.questions[0].category.clone(),
                        quiz.questions[0].difficulty,
                    )
                });
                quiz.setup_category = Some(category);
                quiz.setup_difficulty = Some(difficulty);
                quiz.begin(false);
                assert!(quiz.setup_note.is_some(), "{metrics:?}");
                for page in 0..setup_pages(&quiz, &context).len() {
                    quiz.setup_page = page;
                    let setup = setup_screen(&quiz, &context);
                    let chrome = Chrome::measuring(true);
                    let issues = setup.diagnostics(&metrics, &chrome).issues;
                    assert!(issues.is_empty(), "{metrics:?} page {page}: {issues:?}");
                    assert!(setup
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id("continue-setup"))
                        .is_some());
                    if orientation == Orientation::Portrait && text_scale == TextScale::Default {
                        assert_eq!(banners(&setup).len(), 1, "the note is said on Setup");
                    }
                }
            }
        }
    }
}
