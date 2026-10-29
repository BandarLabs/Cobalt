//! The round in progress, written to the store as it is played.
//!
//! A reader that goes to sleep hands its power back to the Kobo software,
//! which stops every app, so a round that lived only in memory was gone when
//! the cover opened again: the table was back at the start with nobody's
//! scores. Every answer now rewrites a small record of the round, and the app
//! reads it back when it starts.
//!
//! The record carries the round's own questions rather than their places in
//! the bank, because a synced pack can arrive or be replaced between the
//! round being saved and being read, and the questions a table was answering
//! must not change under it.

use super::{Difficulty, Question, Quiz, View, ROUND_LENGTH};

/// The store key the round lives under.
pub(crate) const KEY: &str = "pubquiz-round";

/// The first line of every record. A record from any other version of the
/// format is not guessed at: it is set aside and the app opens at home.
const VERSION: &str = "pubquiz-round 1";

/// A round read back from the store, checked and ready to resume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Saved {
    pub view: View,
    pub party: bool,
    pub players: usize,
    pub question: usize,
    pub player: usize,
    pub scores: [u8; 4],
    pub answers: [Option<usize>; 4],
    pub questions: Vec<Question>,
}

fn stage(view: View) -> Option<&'static str> {
    match view {
        View::Question | View::Choices => Some("question"),
        View::Pass => Some("pass"),
        View::Reveal => Some("reveal"),
        View::Podium => Some("podium"),
        _ => None,
    }
}

/// Tabs and line breaks are the record's own separators. Questions from a
/// synced pack have had their whitespace folded already, so this only guards
/// against a future source that has not.
fn field(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

/// The record for the round on screen, or `None` when there is no round to
/// keep. Rules and About opened from a round keep the round they came from.
pub(crate) fn encode(quiz: &Quiz) -> Option<String> {
    let view = if matches!(quiz.view, View::Rules | View::About) {
        quiz.return_to
    } else {
        quiz.view
    };
    let stage = stage(view)?;
    let scores = quiz.scores.map(|score| score.to_string()).join(",");
    let answers = quiz
        .answers
        .map(|answer| answer.map_or_else(|| "-".to_owned(), |index| index.to_string()))
        .join(",");
    let mut record = format!(
        "{VERSION}\n{stage}\t{}\t{}\t{}\t{}\t{scores}\t{answers}",
        u8::from(quiz.party),
        quiz.players,
        quiz.question,
        quiz.player
    );
    for question in &quiz.round_questions {
        record.push('\n');
        record.push_str(&field(&question.category));
        record.push('\t');
        record.push_str(question.difficulty.key());
        record.push('\t');
        record.push_str(&field(&question.text));
        for answer in &question.answers {
            record.push('\t');
            record.push_str(&field(answer));
        }
        record.push('\t');
        record.push_str(&question.correct.to_string());
    }
    Some(record)
}

fn question(line: &str) -> Option<Question> {
    let fields: Vec<&str> = line.split('\t').collect();
    let [category, difficulty, text, a, b, c, d, correct] = fields.as_slice() else {
        return None;
    };
    let difficulty = match *difficulty {
        "easy" => Difficulty::Easy,
        "medium" => Difficulty::Medium,
        "hard" => Difficulty::Hard,
        _ => return None,
    };
    let correct: usize = correct.parse().ok().filter(|index| *index < 4)?;
    let answers = [a, b, c, d].map(|answer| (*answer).to_owned());
    if category.is_empty() || text.is_empty() || answers.iter().any(String::is_empty) {
        return None;
    }
    Some(Question {
        category: (*category).to_owned(),
        difficulty,
        text: (*text).to_owned(),
        answers,
        correct,
    })
}

/// Reads a record back, or `None` for anything that is not a round this
/// version wrote and could still be playing.
///
/// Every number is checked against the others before any of it is used: a
/// record cut short by a power cut, or edited by hand, should open the app at
/// home rather than on a question that does not exist.
pub(crate) fn decode(bytes: &[u8]) -> Option<Saved> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != VERSION {
        return None;
    }
    let header: Vec<&str> = lines.next()?.split('\t').collect();
    let [stage, party, players, question, player, scores, answers] = header.as_slice() else {
        return None;
    };
    let view = match *stage {
        "question" => View::Question,
        "pass" => View::Pass,
        "reveal" => View::Reveal,
        "podium" => View::Podium,
        _ => return None,
    };
    let party = match *party {
        "1" => true,
        "0" => false,
        _ => return None,
    };
    let players: usize = players.parse().ok().filter(|n| (2..=4).contains(n))?;
    let question: usize = question.parse().ok()?;
    let player: usize = player.parse().ok()?;
    let questions: Vec<Question> = lines.map(self::question).collect::<Option<_>>()?;
    if questions.is_empty() || questions.len() > ROUND_LENGTH {
        return None;
    }
    let finished = view == View::Podium;
    if (finished && question != questions.len()) || (!finished && question >= questions.len()) {
        return None;
    }
    let seats = if party { players } else { 1 };
    if player >= seats {
        return None;
    }
    let scores: Vec<u8> = scores
        .split(',')
        .map(|score| score.parse().ok())
        .collect::<Option<_>>()?;
    let scores: [u8; 4] = scores.try_into().ok()?;
    if scores
        .iter()
        .any(|&score| usize::from(score) > questions.len())
    {
        return None;
    }
    let answers: Vec<Option<usize>> = answers
        .split(',')
        .map(|answer| match answer {
            "-" => Some(None),
            index => index.parse().ok().filter(|i| *i < 4).map(Some),
        })
        .collect::<Option<_>>()?;
    let answers: [Option<usize>; 4] = answers.try_into().ok()?;
    Some(Saved {
        view,
        party,
        players,
        question,
        player,
        scores,
        answers,
        questions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::STATE;
    use kobo_sdk::{action_id, AppRunner, Command, StoreRequest, StoreResult};

    /// The app's last word to the store about the round.
    #[derive(Debug, PartialEq)]
    enum Write {
        Kept(Vec<u8>),
        Forgotten,
    }
    impl Write {
        fn record(self) -> Vec<u8> {
            match self {
                Self::Kept(record) => record,
                Self::Forgotten => panic!("the round was forgotten, not kept"),
            }
        }
    }

    fn last_round_write(commands: &[Command]) -> Option<Write> {
        commands.iter().rev().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == KEY => {
                Some(Write::Kept(value.clone()))
            }
            Command::Store(StoreRequest::Forget { key }) if key == KEY => Some(Write::Forgotten),
            _ => None,
        })
    }

    /// A fresh process, as after the reader slept and woke: nothing in
    /// memory, and the store answering the loads the app asks for at start.
    fn restarted(record: Option<Vec<u8>>) -> AppRunner<Quiz> {
        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: Some(b"0|0|3|Sam,Bo,Cleo,Dev".to_vec()),
        });
        runner.store_result(StoreResult::Loaded {
            key: crate::PACK.into(),
            value: None,
        });
        runner.store_result(StoreResult::Loaded {
            key: KEY.into(),
            value: record,
        });
        runner
    }

    #[test]
    fn a_round_interrupted_by_sleep_comes_back_where_it_stopped() {
        let mut runner = restarted(None);
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("start"));
        let mut written = Vec::new();
        // Sam and Bo answer the first question; Cleo is next.
        let correct = runner.app().round_questions[0].correct;
        written.extend(runner.action(action_id(&format!("answer-{correct}"))));
        written.extend(runner.action(action_id("show-question")));
        written.extend(runner.action(action_id(&format!("answer-{}", (correct + 1) % 4))));
        let before = runner.app();
        assert_eq!(before.view, View::Pass);
        let record = last_round_write(&written)
            .expect("the round was written")
            .record();

        let woken = restarted(Some(record));
        let after = woken.app();
        assert_eq!(after.view, View::Pass);
        assert!(after.party);
        assert_eq!(after.players, 3);
        assert_eq!(after.player, 2);
        assert_eq!(after.question, 0);
        assert_eq!(after.scores, before.scores);
        assert_eq!(after.answers, before.answers);
        assert_eq!(after.round_questions, before.round_questions);
        assert!(format!("{:?}", crate::screen(after)).contains("Pass to Cleo"));
    }

    #[test]
    fn a_solo_round_resumes_on_its_reveal_and_carries_on_to_the_podium() {
        let mut runner = restarted(None);
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        let correct = runner.app().round_questions[0].correct;
        let written = runner.action(action_id(&format!("answer-{correct}")));
        let record = last_round_write(&written).unwrap().record();

        let mut woken = restarted(Some(record));
        assert_eq!(woken.app().view, View::Reveal);
        assert!(!woken.app().party);
        assert_eq!(woken.app().scores[0], 1);
        let total = woken.app().round_questions.len();
        woken.action(action_id("continue"));
        assert_eq!(woken.app().question, 1);
        for _ in 1..total {
            let quiz = woken.app();
            let correct = quiz.round_questions[quiz.question].correct;
            woken.action(action_id(&format!("answer-{correct}")));
            woken.action(action_id("continue"));
        }
        assert_eq!(woken.app().view, View::Podium);
        assert_eq!(usize::from(woken.app().scores[0]), total);
    }

    #[test]
    fn leaving_or_finishing_a_round_forgets_the_saved_copy() {
        let mut runner = restarted(None);
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        runner.action(kobo_sdk::ActionId::BACK);
        let written = runner.action(action_id("leave-round"));
        assert_eq!(last_round_write(&written), Some(Write::Forgotten));
        assert_eq!(runner.app().view, View::Home);

        // Home, with nothing to keep, writes nothing at all.
        let quiet = runner.action(action_id("about"));
        assert_eq!(last_round_write(&quiet), None);
    }

    #[test]
    fn rules_opened_mid_round_still_keep_the_round() {
        let mut runner = restarted(None);
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        let written = runner.action(action_id(kobo_sdk::game::RULES));
        assert_eq!(runner.app().view, View::Rules);
        assert_eq!(last_round_write(&written), None, "nothing about it changed");
        let record = encode(runner.app()).expect("the round is still kept");
        assert_eq!(decode(record.as_bytes()).unwrap().view, View::Question);
    }

    #[test]
    fn a_corrupt_or_old_saved_round_is_ignored_and_forgotten() {
        let mut quiz = Quiz::default();
        quiz.begin(false);
        let good = encode(&quiz).unwrap();
        assert!(decode(good.as_bytes()).is_some());

        let old = good.replacen(VERSION, "pubquiz-round 0", 1);
        let cut = &good[..good.len() / 2];
        let past_the_end = good.replacen("question\t0\t4\t0\t0", "question\t0\t4\t99\t0", 1);
        let wrong_seat = good.replacen("question\t0\t4\t0\t0", "question\t0\t4\t0\t1", 1);
        let impossible_score = good.replacen("\t0,0,0,0\t", "\t200,0,0,0\t", 1);
        let bad_answer = good.replacen("\t-,-,-,-", "\t7,-,-,-", 1);
        assert_ne!(past_the_end, good, "the fixture has the header it edits");
        assert_ne!(wrong_seat, good);
        assert_ne!(impossible_score, good);
        assert_ne!(bad_answer, good);
        for record in [
            old.as_str(),
            cut,
            past_the_end.as_str(),
            wrong_seat.as_str(),
            impossible_score.as_str(),
            bad_answer.as_str(),
            "",
            "pubquiz-round 1",
            "1|5",
        ] {
            assert_eq!(decode(record.as_bytes()), None, "{record:?}");
            let mut runner = AppRunner::new(Quiz::default());
            runner.start();
            let commands = runner.store_result(StoreResult::Loaded {
                key: KEY.into(),
                value: Some(record.as_bytes().to_vec()),
            });
            assert_eq!(runner.app().view, View::Home, "{record:?}");
            assert_eq!(
                last_round_write(&commands),
                Some(Write::Forgotten),
                "{record:?}"
            );
        }
        assert_eq!(decode(&[0xff, 0xfe]), None);
    }

    #[test]
    fn a_late_saved_round_does_not_replace_one_already_started() {
        let mut quiz = Quiz::default();
        quiz.begin(false);
        quiz.answers[0] = Some(0);
        quiz.view = View::Reveal;
        let stale = encode(&quiz).unwrap();

        let mut runner = AppRunner::new(Quiz::default());
        runner.start();
        runner.action(action_id("party"));
        runner.action(action_id("continue-setup"));
        runner.action(action_id("start"));
        runner.store_result(StoreResult::Loaded {
            key: KEY.into(),
            value: Some(stale.into_bytes()),
        });
        assert_eq!(runner.app().view, View::Question);
        assert!(runner.app().party);
    }

    #[test]
    fn waking_from_sleep_asks_the_store_again_only_from_home() {
        let mut runner = restarted(None);
        let asked = |commands: &[Command]| {
            commands.iter().any(|command| {
                matches!(command, Command::Store(StoreRequest::Load { key }) if key == KEY)
            })
        };
        assert!(asked(&runner.resume()));
        runner.action(action_id("solo"));
        runner.action(action_id("continue-setup"));
        assert!(
            !asked(&runner.resume()),
            "a round in memory is already there"
        );
    }
}
