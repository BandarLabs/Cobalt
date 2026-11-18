//! The questions that ship with the app, and the order a round deals them in.
//!
//! The bank lives in a data file rather than in Rust so that adding a
//! question is one line that reads like the question itself, and so that the
//! tests below can hold every line to the same rules a reviewer would.

use super::{Difficulty, Question};
use std::collections::VecDeque;
use std::sync::OnceLock;

/// One question per line: category, difficulty, question, then four answers
/// with the right one marked by a leading `*`.
const SOURCE: &str = include_str!("../assets/questions.tsv");

/// The bundled questions, read from the data file the first time anything
/// asks for them and kept for the life of the process.
pub(crate) fn bundled() -> &'static [Question] {
    static BANK: OnceLock<Vec<Question>> = OnceLock::new();
    BANK.get_or_init(|| {
        SOURCE
            .lines()
            .filter_map(|line| parse_line(line).ok().flatten())
            .collect()
    })
}

/// Reads one line of the data file.
///
/// `Ok(None)` is a comment or a blank line. A line that breaks the format is
/// an error with the reason, which the tests report by line number. The app
/// itself skips such a line rather than refusing to start over one question.
pub(crate) fn parse_line(line: &str) -> Result<Option<Question>, String> {
    let line = line.trim_end_matches('\r');
    if line.trim().is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let fields: Vec<&str> = line.split('\t').collect();
    let [category, difficulty, text, answers @ ..] = fields.as_slice() else {
        return Err("expected a category, a difficulty and a question".to_owned());
    };
    if answers.len() != 4 {
        return Err(format!("expected four answers, found {}", answers.len()));
    }
    let difficulty = match *difficulty {
        "easy" => Difficulty::Easy,
        "medium" => Difficulty::Medium,
        "hard" => Difficulty::Hard,
        other => return Err(format!("unknown difficulty {other:?}")),
    };
    let mut correct = None;
    let mut plain = Vec::with_capacity(4);
    for (index, answer) in answers.iter().enumerate() {
        if let Some(right) = answer.strip_prefix('*') {
            if correct.replace(index).is_some() {
                return Err("more than one answer is marked right".to_owned());
            }
            plain.push(right.trim().to_owned());
        } else {
            plain.push(answer.trim().to_owned());
        }
    }
    let correct = correct.ok_or_else(|| "no answer is marked right".to_owned())?;
    if category.trim().is_empty() || text.trim().is_empty() || plain.iter().any(String::is_empty) {
        return Err("an empty field".to_owned());
    }
    Ok(Some(Question {
        category: category.trim().to_owned(),
        difficulty,
        text: text.trim().to_owned(),
        answers: plain.try_into().expect("four answers were counted"),
        correct,
    }))
}

/// Orders a pool so that any ten in a row mix categories and difficulties.
///
/// The data file is grouped by category and difficulty because that is how a
/// person adds to it, and a round dealt straight from it was ten easy science
/// questions, then ten more. Each category's questions are interleaved by
/// difficulty, each category starting on a different one, and then the
/// categories take turns. A pool of one category and one difficulty keeps its
/// order, so consecutive rounds still deal fresh questions before repeating.
pub(crate) fn dealt(pool: Vec<Question>) -> Vec<Question> {
    let mut categories: Vec<(String, [VecDeque<Question>; 3])> = Vec::new();
    for question in pool {
        let slot = match question.difficulty {
            Difficulty::Easy => 0,
            Difficulty::Medium => 1,
            Difficulty::Hard => 2,
        };
        let index = categories
            .iter()
            .position(|(name, _)| *name == question.category)
            .unwrap_or_else(|| {
                categories.push((question.category.clone(), Default::default()));
                categories.len() - 1
            });
        categories[index].1[slot].push_back(question);
    }
    let mut runs: Vec<VecDeque<Question>> = categories
        .into_iter()
        .enumerate()
        .map(|(index, (_, mut levels))| {
            let mut run = VecDeque::new();
            while levels.iter().any(|level| !level.is_empty()) {
                for step in 0..3 {
                    if let Some(question) = levels[(index + step) % 3].pop_front() {
                        run.push_back(question);
                    }
                }
            }
            run
        })
        .collect();
    let mut order = Vec::new();
    while runs.iter().any(|run| !run.is_empty()) {
        for run in &mut runs {
            if let Some(question) = run.pop_front() {
                order.push(question);
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ROUND_LENGTH;
    use std::collections::{BTreeMap, HashSet};

    const CATEGORIES: [&str; 6] = [
        "Science",
        "Geography",
        "History",
        "Arts & Literature",
        "Nature",
        "Sport & Games",
    ];
    const LEVELS: [Difficulty; 3] = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard];

    #[test]
    fn every_line_of_the_bundled_bank_is_a_well_formed_question() {
        let mut questions = 0;
        for (number, line) in SOURCE.lines().enumerate() {
            match parse_line(line) {
                Ok(Some(_)) => questions += 1,
                Ok(None) => {}
                Err(reason) => panic!("questions.tsv line {}: {reason}: {line}", number + 1),
            }
        }
        assert_eq!(bundled().len(), questions, "the app skips no line");
        assert!(questions >= 400, "only {questions} questions");
    }

    #[test]
    fn every_category_holds_several_rounds_at_every_difficulty() {
        let mut cells = BTreeMap::new();
        for question in bundled() {
            assert!(
                CATEGORIES.contains(&question.category.as_str()),
                "unexpected category {:?}",
                question.category
            );
            *cells
                .entry((question.category.as_str(), question.difficulty.label()))
                .or_insert(0) += 1;
        }
        for category in CATEGORIES {
            for level in LEVELS {
                let count = cells.get(&(category, level.label())).copied().unwrap_or(0);
                assert!(
                    count >= 2 * ROUND_LENGTH,
                    "{category} {} has {count} questions, fewer than two full rounds",
                    level.label()
                );
            }
        }
    }

    #[test]
    fn no_question_is_asked_twice_and_each_has_four_different_answers() {
        let mut seen = HashSet::new();
        for question in bundled() {
            assert!(
                seen.insert(question.text.to_lowercase()),
                "asked twice: {}",
                question.text
            );
            assert!(question.correct < 4, "{}", question.text);
            let answers: HashSet<_> = question
                .answers
                .iter()
                .map(|answer| answer.to_lowercase())
                .collect();
            assert_eq!(answers.len(), 4, "repeated answer in {}", question.text);
        }
    }

    #[test]
    fn questions_and_answers_are_short_enough_for_a_six_inch_screen() {
        for question in bundled() {
            assert!(
                question.text.chars().count() <= 110,
                "question too long: {}",
                question.text
            );
            assert!(question.text.ends_with('?'), "{}", question.text);
            for answer in &question.answers {
                assert!(answer.chars().count() <= 30, "answer too long: {answer}");
            }
        }
    }

    #[test]
    fn the_right_answer_is_spread_across_all_four_places() {
        let mut places = [0_usize; 4];
        for question in bundled() {
            places[question.correct] += 1;
        }
        let total: usize = places.iter().sum();
        for (place, count) in places.iter().enumerate() {
            assert!(
                count * 100 >= total * 15,
                "the right answer is in place {} only {count} times of {total}",
                place + 1
            );
        }
    }

    #[test]
    fn the_original_thirty_questions_come_first_unchanged() {
        let first = &bundled()[0];
        assert_eq!(first.text, "Which planet has the shortest year?");
        assert_eq!(first.answers[first.correct], "Mercury");
        let last = &bundled()[29];
        assert_eq!(
            last.text,
            "Which country hosted the first modern Olympic Games?"
        );
        assert_eq!(last.answers[last.correct], "Greece");
    }

    #[test]
    fn a_malformed_line_says_what_is_wrong_with_it() {
        let two_stars = "Science\teasy\tQ?\t*A\t*B\tC\tD";
        assert!(parse_line(two_stars).unwrap_err().contains("more than one"));
        let no_star = "Science\teasy\tQ?\tA\tB\tC\tD";
        assert!(parse_line(no_star).unwrap_err().contains("no answer"));
        let three = "Science\teasy\tQ?\t*A\tB\tC";
        assert!(parse_line(three).unwrap_err().contains("four answers"));
        let level = "Science\tsilly\tQ?\t*A\tB\tC\tD";
        assert!(parse_line(level).unwrap_err().contains("difficulty"));
        assert_eq!(parse_line("# a comment"), Ok(None));
        assert_eq!(parse_line(""), Ok(None));
        let good = parse_line("Science\thard\tQ?\tA\tB\t*C\tD")
            .unwrap()
            .unwrap();
        assert_eq!(good.correct, 2);
        assert_eq!(good.difficulty, Difficulty::Hard);
    }

    #[test]
    fn the_first_round_from_the_whole_bank_visits_every_category_and_level() {
        let round: Vec<_> = dealt(bundled().to_vec())
            .into_iter()
            .take(ROUND_LENGTH)
            .collect();
        let categories: HashSet<_> = round.iter().map(|q| q.category.as_str()).collect();
        assert_eq!(categories.len(), CATEGORIES.len());
        let levels: HashSet<_> = round.iter().map(|q| q.difficulty.label()).collect();
        assert_eq!(levels.len(), LEVELS.len());

        let science: Vec<_> = bundled()
            .iter()
            .filter(|q| q.category == "Science")
            .cloned()
            .collect();
        let round: Vec<_> = dealt(science).into_iter().take(3).collect();
        let levels: HashSet<_> = round.iter().map(|q| q.difficulty.label()).collect();
        assert_eq!(levels.len(), 3, "one category still mixes its levels");
    }

    #[test]
    fn dealing_keeps_every_question_exactly_once() {
        let bank = bundled().to_vec();
        let mut dealt: Vec<_> = dealt(bank.clone()).into_iter().map(|q| q.text).collect();
        let mut original: Vec<_> = bank.into_iter().map(|q| q.text).collect();
        dealt.sort();
        original.sort();
        assert_eq!(dealt, original);
    }
}
