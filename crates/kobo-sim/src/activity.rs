//! Bounded simulator activity counters. Never retain URLs, credentials, request
//! bodies or returned documents in assertion metadata.

use kobo_protocol::{Message, Task, TaskId, TaskOutcome};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Fetch,
    Post,
    File,
    Sleep,
}

#[derive(Debug)]
pub(super) struct Activity {
    revision: u64,
    callbacks: u64,
    barriers: bool,
    connected: bool,
    active: BTreeMap<TaskId, Kind>,
    effects: [u64; 4],
    completed: u64,
    failed: u64,
    cancelled: u64,
    abandoned: u64,
    cleanup_complete: bool,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            revision: 0,
            callbacks: 1,
            barriers: false,
            connected: true,
            active: BTreeMap::new(),
            effects: [0; 4],
            completed: 0,
            failed: 0,
            cancelled: 0,
            abandoned: 0,
            cleanup_complete: false,
        }
    }
}
impl Activity {
    pub fn started(&mut self, id: TaskId, work: &Task) {
        let (kind, index) = match work {
            Task::Fetch { .. } => (Kind::Fetch, 0),
            Task::Post { .. } => (Kind::Post, 1),
            Task::ReadFile { .. } => (Kind::File, 2),
            Task::Sleep { .. } => (Kind::Sleep, 3),
        };
        self.effects[index] = self.effects[index].saturating_add(1);
        // The runtime bounds concurrent tasks. Refuse to grow metadata even if
        // a malformed client keeps issuing requests with new identities.
        if self.active.len() < 64 && !self.active.contains_key(&id) {
            self.active.insert(id, kind);
        } else {
            self.connected = false;
        }
        self.revision = self.revision.saturating_add(1);
    }
    /// Register an event before it can reach the app. Removing its task and
    /// adding its pending callback happen under the same lock, leaving no idle
    /// gap between network completion and handling the returned document.
    pub fn sent(&mut self, message: &Message) {
        if let Message::TaskOutcome { task, outcome } = message {
            self.active.remove(task);
            match outcome {
                TaskOutcome::Completed(_) => self.completed = self.completed.saturating_add(1),
                TaskOutcome::Failed(_) => self.failed = self.failed.saturating_add(1),
                TaskOutcome::Cancelled => self.cancelled = self.cancelled.saturating_add(1),
            }
        }
        if matches!(
            message,
            Message::Action { .. }
                | Message::TextHold { .. }
                | Message::DeviceResult(_)
                | Message::TaskOutcome { .. }
                | Message::StoreResult(_)
                | Message::Lifecycle(_)
                | Message::ShellEvent(_)
                | Message::CoverChanged { .. }
                | Message::PageTurn { .. }
        ) {
            self.callbacks = self.callbacks.saturating_add(1);
            self.revision = self.revision.saturating_add(1);
        }
    }
    pub fn callback_complete(&mut self) {
        self.barriers = true;
        if self.callbacks == 0 {
            self.connected = false;
        } else {
            self.callbacks -= 1;
        }
        self.revision = self.revision.saturating_add(1);
    }
    pub fn connected(&self) -> bool {
        self.connected
    }
    pub fn finish_disconnect(&mut self) {
        self.abandoned = self.abandoned.saturating_add(self.active.len() as u64);
        self.active.clear();
        self.callbacks = 0;
        self.connected = false;
        self.cleanup_complete = true;
        self.revision = self.revision.saturating_add(1);
    }
    pub fn disconnected(&mut self) {
        self.connected = false;
        self.revision = self.revision.saturating_add(1);
    }
    pub fn json(&self) -> String {
        let sleeping = self
            .active
            .values()
            .filter(|kind| **kind == Kind::Sleep)
            .count();
        let work = self.active.len() - sleeping;
        format!("{{\"revision\":{},\"callbackMarkers\":{},\"connected\":{},\"pendingCallbacks\":{},\"activeWork\":{},\"sleepingTasks\":{},\"idle\":{},\"effects\":{{\"fetch\":{},\"post\":{},\"file\":{},\"sleep\":{}}},\"completed\":{},\"failed\":{},\"cancelled\":{},\"abandoned\":{},\"cleanupComplete\":{}}}", self.revision, self.barriers, self.connected, self.callbacks, work, sleeping, self.barriers && self.connected && self.callbacks == 0 && work == 0, self.effects[0], self.effects[1], self.effects[2], self.effects[3], self.completed, self.failed, self.cancelled, self.abandoned, self.cleanup_complete)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_completion_is_not_idle_until_its_callback_has_finished() {
        let mut activity = Activity::default();
        activity.callback_complete();
        assert!(activity.json().contains("\"idle\":true"));
        activity.sent(&Message::Action {
            action: kobo_ui::ActionId(9),
        });
        activity.started(
            TaskId(1),
            &Task::Fetch {
                url: "https://private.example?token=hidden".into(),
                offset: 0,
                max_bytes: 16,
                credential: None,
                headers: vec![],
            },
        );
        activity.callback_complete();
        assert!(!activity.json().contains("hidden"));
        assert!(activity.json().contains("\"idle\":false"));
        activity.sent(&Message::TaskOutcome {
            task: TaskId(1),
            outcome: TaskOutcome::Completed(b"private body".to_vec()),
        });
        assert!(activity.json().contains("\"activeWork\":0"));
        assert!(activity.json().contains("\"idle\":false"));
        activity.callback_complete();
        assert!(activity.json().contains("\"idle\":true"));
        assert_eq!(activity.json(), activity.json());
        activity.disconnected();
        assert!(activity.json().contains("\"idle\":false"));
    }
    #[test]
    fn scheduled_sleep_is_visible_without_blocking_current_idle() {
        let mut activity = Activity::default();
        activity.started(TaskId(1), &Task::Sleep { seconds: 60 });
        activity.callback_complete();
        assert!(activity.json().contains("\"idle\":true"));
        assert!(activity.json().contains("\"sleepingTasks\":1"));
        activity.sent(&Message::TaskOutcome {
            task: TaskId(1),
            outcome: TaskOutcome::Cancelled,
        });
        assert!(activity.json().contains("\"idle\":false"));
        activity.callback_complete();
        assert!(activity.json().contains("\"cancelled\":1"));
    }
}
