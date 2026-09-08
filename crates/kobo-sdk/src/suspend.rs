//! A save barrier is distinct from a background notification.
use crate::{AppRunner, Command, KoboApp};

#[derive(Debug)]
pub(super) struct Barrier {
    generation: u64,
    replied: bool,
    pub failed: bool,
}

impl<A: KoboApp> AppRunner<A> {
    /// Start one suspend attempt. Repeated/stale generations do not repeat saves.
    pub fn prepare_suspend(&mut self, generation: u64) -> Vec<Command> {
        if generation <= self.suspend_generation || self.suspend_barrier.is_some() {
            return Vec::new();
        }
        self.suspend_generation = generation;
        self.suspend_barrier = Some(Barrier {
            generation,
            replied: false,
            failed: false,
        });
        self.retrying.clear();
        self.dispatch(KoboApp::on_suspend)
    }

    /// End only the matching attempt. A second wake cannot repeat app work.
    pub fn resume_from_suspend(
        &mut self,
        generation: u64,
        _reason: kobo_protocol::WakeReason,
    ) -> Vec<Command> {
        if self
            .suspend_barrier
            .as_ref()
            .is_none_or(|barrier| barrier.generation != generation)
        {
            return Vec::new();
        }
        self.suspend_barrier = None;
        self.displayed = None;
        self.dispatch(KoboApp::on_resume)
    }

    /// Deliver each due occurrence once, only after the host has resumed work.
    pub fn deliver_scheduled_wake(&mut self, occurrence: u64) -> Vec<Command> {
        if occurrence <= self.scheduled_occurrence || self.suspend_barrier.is_some() {
            return Vec::new();
        }
        self.scheduled_occurrence = occurrence;
        self.dispatch(KoboApp::on_scheduled_wake)
    }

    pub(super) fn append_suspend_reply(&mut self, commands: &mut Vec<Command>) {
        if self.outstanding_answers() != 0 || self.tasks_in_flight() != 0 {
            return;
        }
        if let Some(barrier) = &mut self.suspend_barrier {
            if !barrier.replied {
                barrier.replied = true;
                commands.push(Command::SuspendReady {
                    generation: barrier.generation,
                    ready: !barrier.failed && self.app.can_suspend(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActionId, Context, StoreRequest, StoreResult, Task, TaskId, TaskOutcome};

    #[derive(Default)]
    struct DraftApp {
        saves: usize,
        wakes: usize,
        scheduled: usize,
        dirty: bool,
    }
    impl KoboApp for DraftApp {
        fn on_start(&mut self, _: &mut Context) {}
        fn on_action(&mut self, context: &mut Context, _: ActionId) {
            context.spawn(Task::Sleep { seconds: 300 });
        }
        fn on_background(&mut self, context: &mut Context) {
            self.saves += 1;
            context.store().save("draft", b"Garden notes".to_vec());
        }
        fn on_resume(&mut self, _: &mut Context) {
            self.wakes += 1;
        }
        fn on_scheduled_wake(&mut self, _: &mut Context) {
            self.scheduled += 1;
        }
        fn can_suspend(&self) -> bool {
            !self.dirty
        }
    }

    #[test]
    fn readiness_waits_for_both_durable_answers_and_task_cancellation() {
        let mut runner = AppRunner::new(DraftApp::default());
        runner.start();
        runner.action(ActionId(1));
        let preparing = runner.prepare_suspend(1);
        assert!(matches!(
            &preparing[..],
            [Command::Store(StoreRequest::Save { .. })]
        ));
        assert!(runner.prepare_suspend(1).is_empty());
        assert!(runner.prepare_suspend(2).is_empty());
        assert_eq!(runner.app().saves, 1);
        assert!(runner
            .store_result(StoreResult::Saved {
                key: "draft".into()
            })
            .is_empty());
        assert_eq!(
            runner.task_outcome(TaskId(1), TaskOutcome::Cancelled),
            [Command::SuspendReady {
                generation: 1,
                ready: true
            }]
        );
        assert!(runner
            .resume_from_suspend(2, kobo_protocol::WakeReason::Cancelled)
            .is_empty());
        runner.resume_from_suspend(1, kobo_protocol::WakeReason::Cancelled);
        runner.resume_from_suspend(1, kobo_protocol::WakeReason::Cancelled);
        assert_eq!(runner.app().wakes, 1);
        assert!(runner.prepare_suspend(1).is_empty());
        assert_eq!(runner.prepare_suspend(2).len(), 1);
        assert_eq!(runner.app().saves, 2);
    }

    #[test]
    fn scheduled_work_requires_a_targeted_occurrence_after_resume() {
        let mut runner = AppRunner::new(DraftApp::default());
        runner.prepare_suspend(1);
        runner.deliver_scheduled_wake(5);
        assert_eq!(runner.app().scheduled, 0);
        runner.resume_from_suspend(1, kobo_protocol::WakeReason::Scheduled);
        assert_eq!(runner.app().scheduled, 0, "physical wake does not schedule every hosted app");
        runner.deliver_scheduled_wake(5);
        runner.deliver_scheduled_wake(5);
        runner.deliver_scheduled_wake(4);
        assert_eq!(runner.app().scheduled, 1);
        runner.deliver_scheduled_wake(6);
        assert_eq!(runner.app().scheduled, 2);
    }

    #[test]
    fn failed_saves_and_app_owned_dirty_state_refuse_sleep() {
        let mut runner = AppRunner::new(DraftApp::default());
        runner.prepare_suspend(1);
        assert_eq!(
            runner.store_result(StoreResult::Denied(crate::StoreError::NoRoom)),
            [Command::SuspendReady {
                generation: 1,
                ready: false
            }]
        );
        runner.resume_from_suspend(1, kobo_protocol::WakeReason::Cancelled);
        runner.app_mut().dirty = true;
        runner.prepare_suspend(2);
        assert_eq!(
            runner.store_result(StoreResult::Saved {
                key: "draft".into()
            }),
            [Command::SuspendReady {
                generation: 2,
                ready: false
            }]
        );
    }
}
