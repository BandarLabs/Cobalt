//! The questions in flight, shared between the two listeners.
//!
//! A hook thread submits a question and blocks in [`Board::submit`] until
//! somebody answers or its patience runs out -- blocking is the point, since
//! the hook protocol is "write your decision to stdout before you exit".
//! Reader threads wait in [`Board::next`] for something to show, and
//! [`Board::answer`] joins the two. One mutex, two condvars, no channels:
//! the state is small enough to look at whole.

use std::fs::File;
use std::io::Read;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One answer the reader can give, drawn as its own button.
///
/// Both things an agent offers beyond yes and no arrive in this shape: the
/// options of a multiple-choice question, and the "always allow" lines a
/// permission dialog puts under its yes. The reader does not need to know
/// which it is showing, so it is one type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What the button says, and what comes back when it is pressed.
    pub label: String,
    /// The line under it. Empty is fine.
    pub description: String,
}

/// One question a coding agent is waiting on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: u32,
    /// Which agent asked, e.g. `codex` or `claude`.
    pub source: String,
    /// The tool the agent wants to use, e.g. `shell`.
    pub tool: String,
    /// What it wants to do with it, usually the command line.
    pub detail: String,
    /// The answers offered beyond allow and deny. Usually empty.
    pub choices: Vec<Choice>,
}

/// A question on its way to the board: an [`Ask`] without the number, which
/// is the board's to give.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asking {
    pub source: String,
    pub tool: String,
    pub detail: String,
    pub choices: Vec<Choice>,
}

impl Asking {
    /// A plain permission question, answered with allow, deny or neither.
    #[must_use]
    pub fn new(source: &str, tool: &str, detail: &str) -> Self {
        Self {
            source: source.to_owned(),
            tool: tool.to_owned(),
            detail: detail.to_owned(),
            choices: Vec::new(),
        }
    }

    /// The same question with its own answers offered instead.
    #[must_use]
    pub fn offering(mut self, choices: Vec<Choice>) -> Self {
        self.choices = choices;
        self
    }
}

/// What the person on the reader decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    /// No decision: the hook stays silent and the agent falls back to its
    /// own terminal prompt. Also what a timeout or an ignored question
    /// becomes, so an absent reader never blocks anyone.
    Pass,
    /// One of the question's own choices, by label. Never one of the three
    /// above wearing a label, so a choice reading "Allow" is still a choice.
    Chose(String),
}

struct Open {
    ask: Ask,
    decision: Option<Decision>,
}

struct Inner {
    next_id: u32,
    open: Vec<Open>,
}

pub struct Board {
    inner: Mutex<Inner>,
    /// Signalled when a question arrives, waking readers in [`Board::next`].
    asked: Condvar,
    /// Signalled when an answer lands, waking hooks in [`Board::submit`].
    answered: Condvar,
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                next_id: random_start(),
                open: Vec::new(),
            }),
            asked: Condvar::new(),
            answered: Condvar::new(),
        }
    }

    /// Posts a question and blocks until it is answered or `patience` runs
    /// out. The question is removed on the way out either way, so nothing
    /// lingers for the reader to answer into the void.
    pub fn submit(&self, asking: Asking, patience: Duration) -> Decision {
        let deadline = Instant::now() + patience;
        let mut inner = self.lock();
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);
        inner.open.push(Open {
            ask: Ask {
                id,
                source: asking.source,
                tool: asking.tool,
                detail: asking.detail,
                choices: asking.choices,
            },
            decision: None,
        });
        drop(inner);
        self.asked.notify_all();
        let mut inner = self.lock();
        loop {
            let position = inner.open.iter().position(|open| open.ask.id == id);
            let Some(position) = position else {
                // Somebody else removed it; treat as undecided.
                return Decision::Pass;
            };
            if inner.open[position].decision.is_some() {
                let open = inner.open.remove(position);
                return open.decision.unwrap_or(Decision::Pass);
            }
            let now = Instant::now();
            if now >= deadline {
                inner.open.remove(position);
                return Decision::Pass;
            }
            let (guard, _) = self
                .answered
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner = guard;
        }
    }

    /// The oldest unanswered question, waiting up to `patience` for one to
    /// arrive. This is the long-poll body: the reader asks, we hold the line
    /// for a while, and either hand it a question or an empty answer.
    #[must_use]
    pub fn next(&self, patience: Duration) -> Option<Ask> {
        let deadline = Instant::now() + patience;
        let mut inner = self.lock();
        loop {
            if let Some(open) = inner.open.iter().find(|open| open.decision.is_none()) {
                return Some(open.ask.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = self
                .asked
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner = guard;
        }
    }

    /// Records a decision for one question. False when the question is gone
    /// -- already answered and collected, or timed out -- which the reader
    /// reports rather than pretending the tap counted.
    pub fn answer(&self, id: u32, decision: Decision) -> bool {
        let mut inner = self.lock();
        let Some(open) = inner
            .open
            .iter_mut()
            .find(|open| open.ask.id == id && open.decision.is_none())
        else {
            return false;
        };
        open.decision = Some(decision);
        drop(inner);
        self.answered.notify_all();
        true
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

/// Where this run's question numbers start.
///
/// A number is all an answer names, so if every run counted from one, a
/// prompt still on somebody's panel across a daemon restart would name a
/// fresh question it was never about. Starting each run somewhere random
/// makes a stale answer miss -- and be reported as missed -- instead of
/// landing on the wrong question.
fn random_start() -> u32 {
    let mut bytes = [0_u8; 4];
    if let Ok(mut urandom) = File::open("/dev/urandom") {
        if urandom.read_exact(&mut bytes).is_ok() {
            return u32::from_le_bytes(bytes);
        }
    }
    // No urandom to be had: the clock's nanoseconds still miss any earlier
    // run that answered even one question a whole second before this.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    nanos ^ std::process::id()
}

#[cfg(test)]
mod tests {
    use super::{Asking, Board, Choice, Decision};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn an_answer_releases_the_waiting_hook_with_the_decision() {
        let board = Arc::new(Board::new());
        let answering = Arc::clone(&board);
        let answerer = std::thread::spawn(move || {
            let ask = answering.next(Duration::from_secs(5)).expect("a question");
            assert_eq!(ask.source, "codex");
            assert_eq!(ask.detail, "rm -rf ./build");
            assert!(answering.answer(ask.id, Decision::Allow));
        });
        let decision = board.submit(
            Asking::new("codex", "shell", "rm -rf ./build"),
            Duration::from_secs(5),
        );
        assert_eq!(decision, Decision::Allow);
        answerer.join().expect("answerer finishes");
    }

    #[test]
    fn a_hook_nobody_answers_passes_when_its_patience_runs_out() {
        let board = Board::new();
        let decision = board.submit(
            Asking::new("claude", "Bash", "ls"),
            Duration::from_millis(50),
        );
        assert_eq!(decision, Decision::Pass);
    }

    #[test]
    fn a_reader_with_nothing_to_show_gets_none_after_the_wait() {
        let board = Board::new();
        assert_eq!(board.next(Duration::from_millis(50)), None);
    }

    #[test]
    fn answering_a_question_that_timed_out_reports_the_tap_did_not_count() {
        let board = Board::new();
        let _ = board.submit(
            Asking::new("codex", "shell", "ls"),
            Duration::from_millis(10),
        );
        // The board is empty again, so no id can land, least of all this one.
        assert!(!board.answer(1, Decision::Allow));
    }

    #[test]
    fn two_runs_do_not_hand_out_the_same_numbers() {
        // One chance in four billion of a false failure; a stale answer
        // landing on a fresh question after a restart is worth the ticket.
        assert_ne!(super::random_start(), super::random_start());
    }

    #[test]
    fn questions_come_off_the_board_oldest_first() {
        let board = Arc::new(Board::new());
        let first = Arc::clone(&board);
        let hook_one = std::thread::spawn(move || {
            first.submit(
                Asking::new("codex", "shell", "first"),
                Duration::from_secs(5),
            )
        });
        // The second question must arrive after the first is on the board.
        while board.next(Duration::from_millis(10)).is_none() {}
        let second = Arc::clone(&board);
        let hook_two = std::thread::spawn(move || {
            second.submit(
                Asking::new("codex", "shell", "second"),
                Duration::from_secs(5),
            )
        });
        let shown = board.next(Duration::from_secs(5)).expect("a question");
        assert_eq!(shown.detail, "first");
        assert!(board.answer(shown.id, Decision::Deny));
        let shown = board
            .next(Duration::from_secs(5))
            .expect("the next question");
        assert_eq!(shown.detail, "second");
        assert!(board.answer(shown.id, Decision::Allow));
        assert_eq!(hook_one.join().expect("first hook"), Decision::Deny);
        assert_eq!(hook_two.join().expect("second hook"), Decision::Allow);
    }

    #[test]
    fn a_question_carries_its_own_answers_to_the_reader_and_back() {
        let board = Arc::new(Board::new());
        let answering = Arc::clone(&board);
        let answerer = std::thread::spawn(move || {
            let ask = answering.next(Duration::from_secs(5)).expect("a question");
            assert_eq!(ask.choices.len(), 2);
            assert_eq!(ask.choices[1].label, "Detailed");
            assert_eq!(ask.choices[1].description, "Every step");
            assert!(answering.answer(ask.id, Decision::Chose("Detailed".to_owned())));
        });
        let asking = Asking::new("claude", "AskUserQuestion", "How much detail?").offering(vec![
            Choice {
                label: "Summary".to_owned(),
                description: "The short version".to_owned(),
            },
            Choice {
                label: "Detailed".to_owned(),
                description: "Every step".to_owned(),
            },
        ]);
        let decision = board.submit(asking, Duration::from_secs(5));
        assert_eq!(decision, Decision::Chose("Detailed".to_owned()));
        answerer.join().expect("answerer finishes");
    }
}
