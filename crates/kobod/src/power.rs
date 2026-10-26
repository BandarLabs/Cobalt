//! Suspend ownership and races shared by device and simulated hosts.
//! This module issues decisions; it never writes a kernel power interface.
use std::collections::BTreeSet;

/// First wire version that can acknowledge the durable save barrier.
pub const MIN_APP_PROTOCOL: u8 = 14;

pub const PREPARE_TIMEOUT_MILLIS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum State {
    #[default]
    Awake,
    Preparing,
    Ready,
    Suspended,
    Handback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepReason {
    PowerButton,
    Cover,
    Idle,
    Owner,
}

pub use kobo_protocol::WakeReason;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    Busy,
    Charging,
    Usb,
    KeepAwake,
    Terminal,
    Input,
    Panel,
    Save,
    Deadline,
    InvalidApps,
    UnsupportedApp,
    GenerationExhausted,
}

/// Observations must come from the host's real services or explicit fixtures.
#[derive(Clone, Copy, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent host observations, not alternate power states"
)]
pub struct Conditions {
    pub charging: bool,
    pub usb_attached: bool,
    pub keep_awake_until: u64,
    pub terminal_open: bool,
    pub input_quiet: bool,
    pub panel_idle: bool,
    pub tasks_idle: bool,
}

impl Conditions {
    fn refusal(self, now: u64) -> Option<Refusal> {
        if self.usb_attached {
            Some(Refusal::Usb)
        } else if self.charging {
            Some(Refusal::Charging)
        } else if self.keep_awake_until > now {
            Some(Refusal::KeepAwake)
        } else if self.terminal_open {
            Some(Refusal::Terminal)
        } else if !self.input_quiet {
            Some(Refusal::Input)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    Prepare { generation: u64 },
    Enter { generation: u64 },
    Resume { generation: u64, reason: WakeReason },
    Handback { generation: u64 },
}

#[derive(Debug, Default)]
pub struct Power {
    state: State,
    generation: u64,
    waiting: BTreeSet<u64>,
    deadline: u64,
    reason: Option<SleepReason>,
    last_refusal: Option<Refusal>,
    last_wake: Option<WakeReason>,
}

impl Power {
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    #[must_use]
    pub const fn last_refusal(&self) -> Option<Refusal> {
        self.last_refusal
    }
    #[must_use]
    pub const fn last_wake(&self) -> Option<WakeReason> {
        self.last_wake
    }
    #[must_use]
    pub const fn reason(&self) -> Option<SleepReason> {
        self.reason
    }

    /// Begin once for the exact hosted set. The host pauses task admission
    /// before delivering `PrepareSuspend` to every member of this set.
    ///
    /// # Errors
    /// Refuses conflicting ownership, unsafe observations, or an invalid set.
    pub fn begin(
        &mut self,
        apps: &[u64],
        now: u64,
        reason: SleepReason,
        conditions: Conditions,
    ) -> Result<Effect, Refusal> {
        let waiting: BTreeSet<_> = apps.iter().copied().collect();
        let refusal = if self.state != State::Awake {
            Some(Refusal::Busy)
        } else if apps.is_empty()
            || apps.len() > crate::navigation::MAX_HOSTED
            || waiting.len() != apps.len()
            || waiting.contains(&0)
        {
            Some(Refusal::InvalidApps)
        } else {
            conditions.refusal(now)
        };
        if let Some(refusal) = refusal {
            self.last_refusal = Some(refusal);
            return Err(refusal);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(Refusal::GenerationExhausted)?;
        self.waiting = waiting;
        self.deadline = now.saturating_add(PREPARE_TIMEOUT_MILLIS);
        self.state = State::Preparing;
        self.reason = Some(reason);
        self.last_refusal = None;
        Ok(Effect::Prepare {
            generation: self.generation,
        })
    }

    /// Only a current member's first matching reply can satisfy the barrier.
    pub fn acknowledge(&mut self, app: u64, generation: u64, ready: bool) -> Option<Effect> {
        if self.state != State::Preparing
            || generation != self.generation
            || !self.waiting.remove(&app)
        {
            return None;
        }
        if !ready {
            return self.abort(Refusal::Save);
        }
        None
    }

    /// The host calls this after draining results and refreshing observations.
    /// Unverified kernel suspend support selects normal reader handback.
    pub fn poll(
        &mut self,
        now: u64,
        conditions: Conditions,
        suspend_supported: bool,
    ) -> Option<Effect> {
        if self.state != State::Preparing {
            return None;
        }
        if let Some(refusal) = conditions.refusal(now) {
            return self.abort(refusal);
        }
        if now >= self.deadline {
            return self.abort(Refusal::Deadline);
        }
        if !self.waiting.is_empty() || !conditions.tasks_idle || !conditions.panel_idle {
            return None;
        }
        if suspend_supported {
            self.state = State::Ready;
            Some(Effect::Enter {
                generation: self.generation,
            })
        } else {
            self.state = State::Handback;
            Some(Effect::Handback {
                generation: self.generation,
            })
        }
    }

    /// Check again immediately before entering the backend. A wake or abort
    /// during preparation invalidates its outstanding entry decision.
    pub fn entered(&mut self, generation: u64) -> bool {
        if self.state != State::Ready || generation != self.generation {
            return false;
        }
        self.state = State::Suspended;
        true
    }

    pub fn wake(&mut self, reason: WakeReason) -> Option<Effect> {
        if matches!(self.state, State::Awake | State::Handback) {
            return None;
        }
        self.state = State::Awake;
        self.waiting.clear();
        self.last_wake = Some(reason);
        Some(Effect::Resume {
            generation: self.generation,
            reason,
        })
    }

    pub fn abort(&mut self, refusal: Refusal) -> Option<Effect> {
        if matches!(self.state, State::Awake | State::Handback) {
            return None;
        }
        self.last_refusal = Some(refusal);
        self.wake(WakeReason::Cancelled)
    }
}

/// One physical press can cancel preparation or request handback, never both.
#[derive(Debug, Default)]
pub struct Button {
    pressed: bool,
    consumed: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ButtonAction {
    Wake,
    Sleep,
}
impl Button {
    pub fn event(&mut self, pressed: bool, preparing: bool) -> Option<ButtonAction> {
        if pressed {
            if self.pressed {
                return None;
            }
            self.pressed = true;
            self.consumed = preparing;
            preparing.then_some(ButtonAction::Wake)
        } else {
            let was_pressed = std::mem::take(&mut self.pressed);
            let consumed = std::mem::take(&mut self.consumed);
            (was_pressed && !consumed).then_some(ButtonAction::Sleep)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_button_edges_and_cancel_release_cannot_start_another_attempt() {
        let mut button = Button::default();
        assert_eq!(button.event(false, false), None);
        assert_eq!(button.event(true, false), None);
        assert_eq!(button.event(true, false), None);
        assert_eq!(button.event(false, false), Some(ButtonAction::Sleep));
        assert_eq!(button.event(false, true), None);
        assert_eq!(button.event(true, true), Some(ButtonAction::Wake));
        assert_eq!(button.event(true, false), None);
        assert_eq!(button.event(false, false), None);
        assert_eq!(button.event(true, false), None);
        assert_eq!(button.event(false, false), Some(ButtonAction::Sleep));
    }

    fn quiet() -> Conditions {
        Conditions {
            charging: false,
            usb_attached: false,
            keep_awake_until: 0,
            terminal_open: false,
            input_quiet: true,
            panel_idle: true,
            tasks_idle: true,
        }
    }

    #[test]
    fn late_foreign_or_duplicate_answers_cannot_suspend_a_new_attempt() {
        let mut power = Power::default();
        power
            .begin(&[1, 2], 0, SleepReason::Cover, quiet())
            .unwrap();
        power.acknowledge(1, 1, true);
        power.acknowledge(1, 1, true);
        power.acknowledge(9, 1, true);
        assert_eq!(power.poll(1, quiet(), true), None);
        assert_eq!(
            power.wake(WakeReason::Cover),
            Some(Effect::Resume {
                generation: 1,
                reason: WakeReason::Cover
            })
        );
        power
            .begin(&[1, 2], 2, SleepReason::Cover, quiet())
            .unwrap();
        power.acknowledge(2, 1, true);
        assert_eq!(power.poll(3, quiet(), true), None);
        for app in [1, 2] {
            power.acknowledge(app, 2, true);
        }
        assert_eq!(
            power.poll(4, quiet(), true),
            Some(Effect::Enter { generation: 2 })
        );
        assert_eq!(power.poll(4, quiet(), true), None);
        power.wake(WakeReason::Usb);
        assert!(
            !power.entered(2),
            "wake must invalidate the pending entry decision"
        );
        assert_eq!(power.wake(WakeReason::Usb), None);
    }

    #[test]
    fn save_failure_timeout_and_connection_changes_resume_once() {
        for refused in [
            Refusal::Save,
            Refusal::Deadline,
            Refusal::Charging,
            Refusal::Usb,
        ] {
            let mut power = Power::default();
            power
                .begin(&[1], 10, SleepReason::PowerButton, quiet())
                .unwrap();
            let effect = match refused {
                Refusal::Save => power.acknowledge(1, 1, false),
                Refusal::Deadline => power.poll(10 + PREPARE_TIMEOUT_MILLIS, quiet(), true),
                Refusal::Charging => power.poll(
                    11,
                    Conditions {
                        charging: true,
                        ..quiet()
                    },
                    true,
                ),
                _ => power.poll(
                    11,
                    Conditions {
                        usb_attached: true,
                        ..quiet()
                    },
                    true,
                ),
            };
            assert_eq!(
                effect,
                Some(Effect::Resume {
                    generation: 1,
                    reason: WakeReason::Cancelled
                })
            );
            assert_eq!(power.last_refusal(), Some(refused));
            assert_eq!(power.wake(WakeReason::Scheduled), None);
            assert_eq!(power.state(), State::Awake);
        }
    }

    #[test]
    fn actual_worker_and_panel_completion_are_required_even_after_app_ack() {
        let mut power = Power::default();
        power.begin(&[1], 0, SleepReason::Idle, quiet()).unwrap();
        power.acknowledge(1, 1, true);
        assert_eq!(
            power.poll(
                1,
                Conditions {
                    tasks_idle: false,
                    ..quiet()
                },
                true
            ),
            None
        );
        assert_eq!(
            power.poll(
                2,
                Conditions {
                    panel_idle: false,
                    ..quiet()
                },
                true
            ),
            None
        );
        assert_eq!(
            power.poll(3, quiet(), false),
            Some(Effect::Handback { generation: 1 })
        );
        assert!(!power.entered(1));
        assert_eq!(power.wake(WakeReason::Scheduled), None);
    }

    #[test]
    fn waking_twice_never_resumes_the_same_attempt_twice() {
        let mut power = Power::default();
        power.begin(&[1], 0, SleepReason::Owner, quiet()).unwrap();
        power.acknowledge(1, 1, true);
        power.poll(1, quiet(), true);
        assert!(power.entered(1));
        assert_eq!(power.state(), State::Suspended);
        assert!(power.wake(WakeReason::Scheduled).is_some());
        assert_eq!(power.wake(WakeReason::PowerButton), None);
        assert_eq!(power.last_wake(), Some(WakeReason::Scheduled));
    }
}
