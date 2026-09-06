use crate::model::{Card, CardState};
use fsrs::{MemoryState, FSRS};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rating {
    Again,
    Hard,
    Good,
    Easy,
}
impl Rating {
    pub const fn action(self) -> &'static str {
        match self {
            Self::Again => "again",
            Self::Hard => "hard",
            Self::Good => "good",
            Self::Easy => "easy",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preview {
    pub days: u32,
    pub stability: f32,
    pub difficulty: f32,
}
fn state(card: &Card) -> Option<MemoryState> {
    let state = MemoryState {
        stability: card.stability?,
        difficulty: card.difficulty?,
    };
    (state.stability.is_finite() && state.difficulty.is_finite()).then_some(state)
}
fn elapsed(card: &Card, today: i32) -> u32 {
    today.saturating_sub(card.last_review_day).unsigned_abs()
}
pub fn preview(card: &Card, today: i32, rating: Rating) -> Preview {
    let fsrs = FSRS::default();
    let states = fsrs
        .next_states(state(card), 0.9, elapsed(card, today))
        .or_else(|_| fsrs.next_states(None, 0.9, elapsed(card, today)))
        .expect("FSRS default parameters are valid");
    let choice = match rating {
        Rating::Again => states.again,
        Rating::Hard => states.hard,
        Rating::Good => states.good,
        Rating::Easy => states.easy,
    };
    Preview {
        days: format!("{:.0}", choice.interval.round().max(1.0))
            .parse()
            .unwrap_or(u32::MAX),
        stability: choice.memory.stability,
        difficulty: choice.memory.difficulty,
    }
}
pub fn answer(card: &mut Card, today: i32, rating: Rating) -> Preview {
    let next = preview(card, today, rating);
    card.last_review_day = today;
    card.due_day = today.saturating_add(i32::try_from(next.days).unwrap_or(i32::MAX));
    card.stability = Some(next.stability);
    card.difficulty = Some(next.difficulty);
    card.reps = card.reps.saturating_add(1);
    if rating == Rating::Again {
        card.lapses = card.lapses.saturating_add(1);
        card.state = CardState::Learning;
    } else {
        card.state = CardState::Review;
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Card;
    fn card() -> Card {
        Card {
            id: 1,
            deck: "Default".to_owned(),
            front: "Q".to_owned(),
            back: "A".to_owned(),
            last_review_day: 100,
            due_day: 100,
            state: CardState::New,
            reps: 0,
            lapses: 0,
            stability: None,
            difficulty: None,
            media: 0,
        }
    }
    #[test]
    fn fsrs_answer_persists_its_memory_state_and_interval() {
        let mut card = card();
        let expected = preview(&card, 100, Rating::Good);
        let actual = answer(&mut card, 100, Rating::Good);
        assert_eq!(actual, expected);
        assert_eq!(
            card.due_day,
            100 + i32::try_from(expected.days).unwrap_or(i32::MAX)
        );
        assert_eq!(card.reps, 1);
        assert_eq!(card.state, CardState::Review);
        assert!(card.stability.is_some());
    }
    #[test]
    fn again_enters_learning_and_counts_a_lapse() {
        let mut card = card();
        answer(&mut card, 100, Rating::Again);
        assert_eq!(card.state, CardState::Learning);
        assert_eq!(card.lapses, 1);
    }

    #[test]
    fn malformed_memory_falls_back_to_a_new_card_schedule() {
        let mut malformed = card();
        malformed.stability = Some(f32::NAN);
        malformed.difficulty = Some(5.0);
        assert_eq!(
            preview(&malformed, 100, Rating::Good),
            preview(&card(), 100, Rating::Good)
        );
    }
}
