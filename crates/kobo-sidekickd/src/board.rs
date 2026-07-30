//! The questions in flight, shared between the two listeners.
//!
//! A hook thread submits a question and blocks in [`Board::submit`] until
//! somebody answers or its patience runs out -- blocking is the point, since
//! the hook protocol is "write your decision to stdout before you exit".
//! Reader threads wait in [`Board::next`] for something to show, and
//! [`Board::answer`] joins the two. One mutex, two condvars, no channels:
//! the state is small enough to look at whole.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

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
}

/// What the person on the reader decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    /// No decision: the hook stays silent and the agent falls back to its
    /// own terminal prompt. Also what a timeout or an ignored question
    /// becomes, so an absent reader never blocks anyone.
    Pass,
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
                next_id: 1,
                open: Vec::new(),
            }),
            asked: Condvar::new(),
            answered: Condvar::new(),
        }
    }

    /// Posts a question and blocks until it is answered or `patience` runs
    /// out. The question is removed on the way out either way, so nothing
    /// lingers for the reader to answer into the void.
    pub fn submit(&self, source: &str, tool: &str, detail: &str, patience: Duration) -> Decision {
        let deadline = Instant::now() + patience;
        let mut inner = self.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.open.push(Open {
            ask: Ask {
                id,
                source: source.to_owned(),
                tool: tool.to_owned(),
                detail: detail.to_owned(),
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
            if let Some(decision) = inner.open[position].decision {
                inner.open.remove(position);
                return decision;
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

#[cfg(test)]
mod tests {
    use super::{Board, Decision};
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
        let decision = board.submit("codex", "shell", "rm -rf ./build", Duration::from_secs(5));
        assert_eq!(decision, Decision::Allow);
        answerer.join().expect("answerer finishes");
    }

    #[test]
    fn a_hook_nobody_answers_passes_when_its_patience_runs_out() {
        let board = Board::new();
        let decision = board.submit("claude", "Bash", "ls", Duration::from_millis(50));
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
        let _ = board.submit("codex", "shell", "ls", Duration::from_millis(10));
        assert!(!board.answer(1, Decision::Allow));
    }

    #[test]
    fn questions_come_off_the_board_oldest_first() {
        let board = Arc::new(Board::new());
        let first = Arc::clone(&board);
        let hook_one = std::thread::spawn(move || {
            first.submit("codex", "shell", "first", Duration::from_secs(5))
        });
        // The second question must arrive after the first is on the board.
        while board.next(Duration::from_millis(10)).is_none() {}
        let second = Arc::clone(&board);
        let hook_two = std::thread::spawn(move || {
            second.submit("codex", "shell", "second", Duration::from_secs(5))
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
}
