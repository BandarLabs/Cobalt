//! Changes made on the Kobo, kept until Miniflux has acknowledged them.
//!
//! Reading happens where the Wi-Fi is not, so every change starts as a local
//! fact and becomes a server fact later. The queue therefore has to survive a
//! restart, which means it is written down and counted as written only once
//! the store says so.
//!
//! Every change here is an assignment: read, unread, starred, not starred,
//! archived. That is what makes a lost reply harmless. The runtime sends an
//! update once and never replays it, since a reply can go missing after the
//! change has been applied, so a queue of toggles would be impossible to
//! resume safely. A queue of assignments is simply sent again.
use kobo_sdk::{Context, StoreResult};
use std::collections::VecDeque;
use std::fmt::Write;

pub const KEY: &str = "read-actions";
const LIMIT: usize = 64 * 1024;
const MAX_CHANGES: usize = 500;
const FORMAT: &str = "miniflux-changes-v1\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Read,
    Unread,
    Star,
    Unstar,
    Archive,
}

impl Change {
    const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Unread => "unread",
            Self::Star => "star",
            Self::Unstar => "unstar",
            Self::Archive => "archive",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [
            Self::Read,
            Self::Unread,
            Self::Star,
            Self::Unstar,
            Self::Archive,
        ]
        .into_iter()
        .find(|change| change.name() == name)
    }

    /// The change that undoes this one while it is still only local.
    const fn opposite(self) -> Option<Self> {
        match self {
            Self::Read => Some(Self::Unread),
            Self::Unread => Some(Self::Read),
            Self::Star => Some(Self::Unstar),
            Self::Unstar => Some(Self::Star),
            Self::Archive => None,
        }
    }
}

#[derive(Default)]
pub struct Pending {
    queue: VecDeque<(u64, Change)>,
    saved: Vec<u8>,
    writing: Option<Vec<u8>>,
    /// Whether the change at the head is in flight. Only one is ever sent at
    /// a time, so the order the reader made them in is the order Miniflux
    /// receives them in.
    sending: bool,
    loaded: bool,
    /// The queue itself could not be written.
    pub failed: bool,
}

impl Pending {
    /// Reads the queue back, including the one Feeds' predecessor wrote.
    ///
    /// The first version of this application saved read marks as a list of
    /// identifiers with nothing else in it. Somebody who marked articles read
    /// on a train and then updated should still have those marks, so that
    /// shape is accepted and understood as what it was: read marks.
    pub fn load(&mut self, result: &StoreResult) {
        let StoreResult::Loaded { value, .. } = result else {
            self.failed = true;
            return;
        };
        let bytes = value.clone().unwrap_or_default();
        let Some(queue) = decode(&bytes) else {
            self.failed = true;
            return;
        };
        self.queue = queue;
        self.saved = bytes;
        self.loaded = true;
        self.failed = false;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Every local change to one article, oldest first.
    pub fn changes_to(&self, id: u64) -> impl Iterator<Item = Change> + '_ {
        self.queue
            .iter()
            .filter(move |(article, _)| *article == id)
            .map(|(_, change)| *change)
    }

    /// Records a change the reader made.
    ///
    /// A change that undoes one still waiting removes it instead of queueing
    /// its opposite: starring and unstarring while offline should reach the
    /// server as nothing at all rather than as two toggles whose order decides
    /// the answer. The change being sent right now is never collapsed, because
    /// its reply is still owed and the queue has to be able to interpret it.
    pub fn push(&mut self, context: &mut Context, id: u64, change: Change) -> bool {
        if !self.loaded || self.queue.len() >= MAX_CHANGES {
            self.failed = true;
            return false;
        }
        let settled = usize::from(self.sending);
        if let Some(undone) = change.opposite() {
            if let Some(index) = self
                .queue
                .iter()
                .enumerate()
                .skip(settled)
                .rev()
                .find(|(_, queued)| **queued == (id, undone))
                .map(|(index, _)| index)
            {
                self.queue.remove(index);
                self.flush(context);
                return true;
            }
        }
        if self.queue.iter().skip(settled).any(|q| *q == (id, change)) {
            return true;
        }
        self.queue.push_back((id, change));
        self.flush(context);
        true
    }

    /// The change to send next, if one may be sent at all.
    #[must_use]
    pub fn next_change(&self) -> Option<(u64, Change)> {
        if self.sending {
            return None;
        }
        self.queue.front().copied()
    }

    pub fn begin(&mut self) {
        self.sending = true;
    }

    /// The server confirmed the change at the head.
    pub fn applied(&mut self, context: &mut Context) {
        self.sending = false;
        self.queue.pop_front();
        self.flush(context);
    }

    /// The reply never arrived, so the change stays at the head of the queue.
    ///
    /// It may already have been applied. Because it is an assignment, that
    /// costs nothing: the next attempt asks for the same end state.
    pub fn unresolved(&mut self) {
        self.sending = false;
    }

    pub fn retry(&mut self, context: &mut Context) {
        if !self.loaded {
            context.store().load(KEY);
            return;
        }
        self.failed = false;
        self.flush(context);
    }

    fn flush(&mut self, context: &mut Context) {
        let bytes = encode(&self.queue);
        if bytes.len() > LIMIT {
            self.failed = true;
            return;
        }
        if self.writing.is_none() && bytes != self.saved {
            context.store().save(KEY, bytes.clone());
            self.writing = Some(bytes);
        }
    }

    pub fn stored(&mut self, context: &mut Context, result: &StoreResult) {
        let Some(bytes) = self.writing.take() else {
            return;
        };
        if matches!(result, StoreResult::Saved { key } if key == KEY) {
            self.saved = bytes;
            self.failed = false;
            self.flush(context);
        } else {
            self.failed = true;
        }
    }
}

fn encode(queue: &VecDeque<(u64, Change)>) -> Vec<u8> {
    let mut out = String::from(FORMAT);
    for (id, change) in queue {
        let _ = writeln!(out, "{id}\t{}", change.name());
    }
    out.into_bytes()
}

fn decode(bytes: &[u8]) -> Option<VecDeque<(u64, Change)>> {
    if bytes.len() > LIMIT {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    if text.trim().is_empty() {
        return Some(VecDeque::new());
    }
    let mut queue = VecDeque::new();
    match text.strip_prefix(FORMAT) {
        Some(lines) => {
            for line in lines.lines() {
                let (id, name) = line.split_once('\t')?;
                queue.push_back((id.parse().ok()?, Change::from_name(name)?));
            }
        }
        // The first version's list of read identifiers.
        None => {
            for id in text.trim().split(',').filter(|id| !id.is_empty()) {
                queue.push_back((id.parse().ok()?, Change::Read));
            }
        }
    }
    if queue.len() > MAX_CHANGES {
        return None;
    }
    Some(queue)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(value: Option<Vec<u8>>) -> Pending {
        let mut pending = Pending::default();
        pending.load(&StoreResult::Loaded {
            key: KEY.into(),
            value,
        });
        pending
    }

    #[test]
    fn read_marks_written_by_the_first_version_are_still_understood() {
        let pending = loaded(Some(b"7,9".to_vec()));
        assert_eq!(
            pending.queue,
            VecDeque::from([(7, Change::Read), (9, Change::Read)])
        );
        assert!(loaded(Some(b"not a queue".to_vec())).failed);
    }

    #[test]
    fn undoing_a_waiting_change_removes_it_rather_than_queueing_its_opposite() {
        let mut context = Context::default();
        let mut pending = loaded(None);
        assert!(pending.push(&mut context, 7, Change::Star));
        assert!(pending.push(&mut context, 7, Change::Unstar));
        assert_eq!(pending.len(), 0);
        assert!(pending.push(&mut context, 7, Change::Read));
        assert!(pending.push(&mut context, 7, Change::Read));
        assert_eq!(pending.len(), 1, "the same change was queued twice");
    }

    #[test]
    fn a_change_being_sent_is_never_collapsed_by_a_later_one() {
        let mut context = Context::default();
        let mut pending = loaded(None);
        pending.push(&mut context, 7, Change::Star);
        assert_eq!(pending.next_change(), Some((7, Change::Star)));
        pending.begin();
        assert_eq!(pending.next_change(), None, "two changes were sent at once");
        pending.push(&mut context, 7, Change::Unstar);
        assert_eq!(pending.len(), 2);
        pending.applied(&mut context);
        assert_eq!(pending.next_change(), Some((7, Change::Unstar)));
    }

    #[test]
    fn a_lost_reply_leaves_the_change_at_the_head_to_be_sent_again() {
        let mut context = Context::default();
        let mut pending = loaded(None);
        pending.push(&mut context, 7, Change::Read);
        pending.push(&mut context, 8, Change::Star);
        pending.begin();
        pending.unresolved();
        assert_eq!(
            pending.next_change(),
            Some((7, Change::Read)),
            "the unanswered change lost its place in the queue"
        );
        pending.applied(&mut context);
        assert_eq!(pending.next_change(), Some((8, Change::Star)));
    }

    #[test]
    fn the_queue_is_written_once_and_again_only_after_it_is_acknowledged() {
        let mut context = Context::default();
        let mut pending = loaded(None);
        pending.push(&mut context, 7, Change::Read);
        assert!(pending.writing.is_some());
        let first = pending.writing.clone().unwrap();
        pending.push(&mut context, 8, Change::Read);
        assert_eq!(
            pending.writing.as_ref(),
            Some(&first),
            "a second write started before the first was acknowledged"
        );
        pending.stored(&mut context, &StoreResult::Saved { key: KEY.into() });
        assert_eq!(decode(pending.writing.as_ref().unwrap()).unwrap().len(), 2);
        pending.stored(
            &mut context,
            &StoreResult::Denied(kobo_sdk::StoreError::NoRoom),
        );
        assert!(pending.failed);
    }
}
