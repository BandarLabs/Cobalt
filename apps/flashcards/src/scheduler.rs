//! FSRS scheduling on the reader.
//!
//! The computer prepares a day's queue, but a reader that cannot work out
//! the next day for itself runs dry the morning after every transfer. Each
//! grade given here is turned into a next due day with the same FSRS model
//! Anki uses, so cards come back on their own until the next push.

use crate::Grade;
use fsrs::{MemoryState, FSRS};
use kobo_flashcards_format::Card;

/// The recall FSRS aims for when it picks a gap. It is Anki's default, so a
/// card graded here comes back about when Anki would have brought it back.
const DESIRED_RETENTION: f32 = 0.9;

/// Anki's default ceiling on an interval, a hundred years.
const MAX_INTERVAL_DAYS: f32 = 36_500.0;

/// Where one card stands after the reader last graded it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    pub stability: f32,
    pub difficulty: f32,
    pub last_day: i64,
    pub due_day: i64,
    /// Answered Again on `last_day`. Such a card is due the same day, behind
    /// every card not yet seen, the way a missed card comes back later in an
    /// Anki session instead of waiting for tomorrow.
    pub again: bool,
}

/// What the reader knows about a card before it grades the card itself:
/// the memory Anki left, and when Anki last reviewed it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prior {
    pub memory: Option<(f32, f32)>,
    pub last_day: Option<i64>,
    /// Anki's own gap, used for the time since the last review when the day
    /// of that review cannot be placed on the reader's calendar.
    pub interval: u32,
}

impl Prior {
    /// Reads a card's Anki history. A collection studied with FSRS keeps the
    /// memory state in the card's data; one studied with the older SM-2
    /// scheduler has an ease and an interval, which FSRS converts. A new or
    /// learning card has no memory yet and starts from FSRS's first step.
    pub fn of(card: &Card, due_day: Option<i64>) -> Self {
        let reviewed = matches!(card.card_type, 2 | 3) && card.interval > 0;
        let interval = if reviewed {
            card.interval.unsigned_abs()
        } else {
            0
        };
        let memory = fsrs_memory(&card.data).or_else(|| {
            if !reviewed || card.ease_factor <= 0 {
                return None;
            }
            #[allow(
                clippy::cast_precision_loss,
                reason = "an Anki ease in permille and an interval in days are both far below 2^24"
            )]
            let (ease, days) = (card.ease_factor as f32 / 1000.0, interval as f32);
            FSRS::default()
                .memory_state_from_sm2(ease, days, DESIRED_RETENTION)
                .ok()
                .map(|state| (state.stability, state.difficulty))
        });
        Self {
            memory,
            last_day: due_day
                .filter(|_| reviewed)
                .map(|due| due.saturating_sub(i64::from(interval))),
            interval,
        }
    }
}

/// Grades a card on `today` and says when it is next due.
pub fn answer(previous: Option<&Schedule>, prior: Prior, today: i64, grade: Grade) -> Schedule {
    let (memory, elapsed) = match previous {
        Some(schedule) => (
            Some((schedule.stability, schedule.difficulty)),
            days_between(schedule.last_day, today),
        ),
        None => (
            prior.memory,
            prior
                .last_day
                .map_or(prior.interval, |day| days_between(day, today)),
        ),
    };
    let memory = memory
        .map(|(stability, difficulty)| MemoryState {
            stability,
            difficulty,
        })
        .filter(|state| state.stability.is_finite() && state.difficulty.is_finite());
    let fsrs = FSRS::default();
    let states = fsrs
        .next_states(memory, DESIRED_RETENTION, elapsed)
        .or_else(|_| fsrs.next_states(None, DESIRED_RETENTION, 0))
        .expect("FSRS default parameters are valid");
    let state = match grade {
        Grade::Again => states.again,
        Grade::Hard => states.hard,
        Grade::Good => states.good,
        Grade::Easy => states.easy,
    };
    let gap = if grade == Grade::Again {
        0
    } else {
        whole_days(state.interval)
    };
    Schedule {
        stability: state.memory.stability,
        difficulty: state.memory.difficulty,
        last_day: today,
        due_day: today.saturating_add(gap),
        again: grade == Grade::Again,
    }
}

fn days_between(from: i64, to: i64) -> u32 {
    u32::try_from(to.saturating_sub(from).max(0)).unwrap_or(u32::MAX)
}

/// An FSRS interval as whole days, at least one: a card answered correctly
/// is never due again the same day.
fn whole_days(interval: f32) -> i64 {
    let days = if interval.is_finite() {
        interval.round().clamp(1.0, MAX_INTERVAL_DAYS)
    } else {
        1.0
    };
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the value is a whole number between 1 and 36500"
    )]
    let days = days as i64;
    days
}

/// The `"s"` and `"d"` numbers Anki writes into a card's data once FSRS is on.
fn fsrs_memory(data: &str) -> Option<(f32, f32)> {
    let number = |key: &str| -> Option<f32> {
        let start = data.find(key)? + key.len();
        let digits: String = data[start..]
            .trim_start()
            .chars()
            .take_while(|character| {
                character.is_ascii_digit() || matches!(character, '.' | '-' | 'e' | 'E' | '+')
            })
            .collect();
        digits
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0)
    };
    Some((number("\"s\":")?, number("\"d\":")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(card_type: i32, interval: i32, ease_factor: i32, data: &str) -> Card {
        Card {
            id: 1,
            note_id: 1,
            deck_id: 1,
            ordinal: 0,
            user_sequence: 0,
            queue: card_type,
            card_type,
            due: 0,
            interval,
            ease_factor,
            repetitions: 0,
            lapses: 0,
            remaining_steps: 0,
            original_due: 0,
            original_deck_id: 0,
            flags: 0,
            data: data.to_owned(),
            modified: 0,
            template_name: String::new(),
            front: String::new(),
            back: String::new(),
            front_spans: Vec::new(),
            back_spans: Vec::new(),
            tags: Vec::new(),
            question_media_names: Vec::new(),
            answer_media_names: Vec::new(),
            media_names: Vec::new(),
            attachments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn a_missed_card_is_due_again_the_same_day() {
        let missed = answer(None, Prior::default(), 100, Grade::Again);
        assert_eq!(missed.due_day, 100);
        assert!(missed.again);
        let recovered = answer(Some(&missed), Prior::default(), 100, Grade::Good);
        assert!(recovered.due_day > 100);
        assert!(!recovered.again);
    }

    #[test]
    fn a_remembered_card_waits_longer_the_easier_it_was() {
        let hard = answer(None, Prior::default(), 100, Grade::Hard);
        let good = answer(None, Prior::default(), 100, Grade::Good);
        let easy = answer(None, Prior::default(), 100, Grade::Easy);
        assert!(hard.due_day >= 101);
        assert!(hard.due_day <= good.due_day);
        assert!(good.due_day < easy.due_day);
    }

    #[test]
    fn a_second_success_waits_longer_than_the_first() {
        let first = answer(None, Prior::default(), 100, Grade::Good);
        let second = answer(Some(&first), Prior::default(), first.due_day, Grade::Good);
        assert!(second.due_day - first.due_day > first.due_day - 100);
    }

    #[test]
    fn an_anki_review_card_continues_from_its_sm2_ease_and_interval() {
        let mature = card(2, 30, 2500, "");
        let prior = Prior::of(&mature, Some(130));
        assert_eq!(prior.last_day, Some(100));
        assert!(prior.memory.is_some());
        let graded = answer(None, prior, 130, Grade::Good);
        let fresh = answer(None, Prior::default(), 130, Grade::Good);
        assert!(graded.due_day > fresh.due_day + 10);
    }

    #[test]
    fn an_anki_fsrs_card_keeps_the_memory_anki_stored() {
        let studied = card(2, 10, 2500, r#"{"s":42.5,"d":6.25,"dr":0.9}"#);
        assert_eq!(Prior::of(&studied, None).memory, Some((42.5, 6.25)));
        assert_eq!(Prior::of(&card(0, 0, 0, ""), None), Prior::default());
    }

    #[test]
    fn a_malformed_memory_falls_back_to_a_first_review() {
        let broken = Schedule {
            stability: f32::NAN,
            difficulty: 5.0,
            last_day: 90,
            due_day: 100,
            again: false,
        };
        assert_eq!(
            answer(Some(&broken), Prior::default(), 100, Grade::Good).due_day,
            answer(None, Prior::default(), 100, Grade::Good).due_day
        );
    }
}
