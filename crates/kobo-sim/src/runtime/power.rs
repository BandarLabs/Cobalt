//! Real SDK save barriers with explicitly simulated power entry.
use super::Hosted;
use kobo_protocol::{Frame, Message};
use kobod::power::{Conditions, Effect, Power, Refusal, SleepReason, State, WakeReason};
use std::io;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Input {
    Sleep(SleepReason),
    Wake(WakeReason),
    Usb(bool),
}
impl Input {
    pub fn parse(command: &str) -> Option<Self> {
        Some(match command {
            "sleep" => Self::Sleep(SleepReason::Owner),
            "sleep cover" => Self::Sleep(SleepReason::Cover),
            "sleep idle" => Self::Sleep(SleepReason::Idle),
            "wake" | "wake power" => Self::Wake(WakeReason::PowerButton),
            "wake cover" => Self::Wake(WakeReason::Cover),
            "wake touch" => Self::Wake(WakeReason::Touch),
            "wake scheduled" => Self::Wake(WakeReason::Scheduled),
            "usb attach" => Self::Usb(true),
            "usb detach" => Self::Usb(false),
            _ => return None,
        })
    }
}

#[derive(Default)]
pub(super) struct Controller {
    power: Power,
    frontlight: Option<u8>,
    usb: bool,
    refusal: Option<String>,
    scheduled_occurrence: u64,
}

impl Controller {
    pub fn awake(&self) -> bool {
        self.power.state() == State::Awake
    }

    pub fn cancel(&mut self, apps: &mut [Hosted]) -> io::Result<()> {
        if let Some(effect) = self.power.abort(Refusal::Busy) {
            self.apply(apps, effect)?;
        }
        Ok(())
    }

    pub fn step(&mut self, apps: &mut [Hosted], front: u64) -> io::Result<()> {
        let index = apps
            .iter()
            .position(|app| app.id == front)
            .ok_or_else(|| io::Error::other("foreground lost"))?;
        let (now, request, hardware) = {
            let mut state = apps[index].session.state.lock().map_err(lock_error)?;
            (
                state.time.now()?.monotonic_millis,
                state.power_request.take(),
                state.effective_hardware(),
            )
        };
        let conditions = self.conditions(apps, front, now)?;
        if let Some(request) = request {
            let scheduled = matches!(request, Input::Wake(WakeReason::Scheduled)) && !self.awake();
            self.request(apps, request, now, conditions)?;
            if scheduled && self.awake() {
                self.deliver_scheduled(apps, front)?;
            }
        }
        if hardware.charging {
            if let Some(effect) = self.power.wake(WakeReason::Charging) {
                self.apply(apps, effect)?;
            }
        }
        for index in 0..apps.len() {
            let (answer, scheduled) = {
                let mut state = apps[index].session.state.lock().map_err(lock_error)?;
                let scheduled = state.scheduled_wake.is_some_and(|deadline| now >= deadline);
                if scheduled {
                    state.scheduled_wake = None;
                }
                (state.suspend_reply.take(), scheduled)
            };
            if let Some((generation, ready)) = answer {
                if let Some(effect) = self.power.acknowledge(apps[index].id, generation, ready) {
                    self.apply(apps, effect)?;
                }
            }
            if scheduled {
                if let Some(effect) = self.power.wake(WakeReason::Scheduled) {
                    self.apply(apps, effect)?;
                }
                self.deliver_scheduled(apps, apps[index].id)?;
            }
        }
        let conditions = self.conditions(apps, front, now)?;
        if let Some(effect) = self.power.poll(now, conditions, true) {
            if matches!(effect, Effect::Enter { .. }) {
                self.frontlight = Some(hardware.frontlight_percent);
            }
            self.apply(apps, effect)?;
        }
        let status = kobo_json::ObjectBuilder::new()
            .set("mode", "simulated-entry-with-real-sdk-barrier")
            .set("hardwareValidated", false)
            .set("state", format!("{:?}", self.power.state()).to_lowercase())
            .set("generation", self.power.generation().to_string())
            .set("usbAttached", self.usb)
            .set("reason", format!("{:?}", self.power.reason()))
            .set("lastWake", format!("{:?}", self.power.last_wake()))
            .set(
                "lastRefusal",
                self.refusal
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", self.power.last_refusal())),
            )
            .build();
        for app in apps {
            let mut state = app.session.state.lock().map_err(lock_error)?;
            state.power_status = status.clone();
            state.power_state = self.power.state();
            state.power_generation = self.power.generation();
        }
        Ok(())
    }

    fn deliver_scheduled(&mut self, apps: &[Hosted], owner: u64) -> io::Result<()> {
        self.scheduled_occurrence = self
            .scheduled_occurrence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("scheduled occurrences exhausted"))?;
        if let Some(app) = apps
            .iter()
            .find(|app| app.id == owner && app.session.writer.connected())
        {
            crate::write_shared(
                &app.session.writer,
                &Frame {
                    version: kobo_protocol::VERSION,
                    request_id: 0,
                    message: Message::ScheduledWake {
                        occurrence: self.scheduled_occurrence,
                    },
                },
            )?;
        }
        Ok(())
    }

    fn request(
        &mut self,
        apps: &mut [Hosted],
        request: Input,
        now: u64,
        conditions: Conditions,
    ) -> io::Result<()> {
        let effect = match request {
            Input::Sleep(reason) => {
                if apps.iter().any(|app| {
                    app.session
                        .state
                        .lock()
                        .map_or(true, |state| state.protocol < kobo_protocol::VERSION)
                }) {
                    self.refusal =
                        Some("An app needs updating before it can acknowledge sleep".into());
                    return Ok(());
                }
                self.refusal = None;
                self.power
                    .begin(
                        &apps.iter().map(|app| app.id).collect::<Vec<_>>(),
                        now,
                        reason,
                        conditions,
                    )
                    .ok()
            }
            Input::Wake(reason) => self.power.wake(reason),
            Input::Usb(attached) => {
                self.usb = attached;
                attached.then(|| self.power.wake(WakeReason::Usb)).flatten()
            }
        };
        if let Some(effect) = effect {
            self.apply(apps, effect)?;
        }
        Ok(())
    }

    fn conditions(&self, apps: &[Hosted], front: u64, now: u64) -> io::Result<Conditions> {
        let mut conditions = Conditions {
            charging: false,
            usb_attached: self.usb,
            keep_awake_until: 0,
            terminal_open: false,
            input_quiet: true,
            panel_idle: true,
            tasks_idle: true,
        };
        for app in apps {
            let tasks = {
                let state = app.session.state.lock().map_err(lock_error)?;
                if app.id == front {
                    conditions.charging = state.effective_hardware().charging;
                    conditions.input_quiet = state.input.is_quiescent();
                    conditions.panel_idle = state.panel.accepts_input();
                }
                conditions.terminal_open |= state.terminal_open;
                conditions.keep_awake_until = conditions.keep_awake_until.max(state.wake_until);
                state.tasks.clone()
            };
            if let Some(tasks) = tasks {
                let tasks = tasks.lock().map_err(lock_error)?;
                conditions.tasks_idle &= if self.awake() {
                    tasks.in_flight() == 0
                } else {
                    tasks.is_quiescent()
                };
            }
        }
        // Keep expired leases as observations without renewing them on polling.
        conditions.keep_awake_until = conditions.keep_awake_until.max(now);
        Ok(conditions)
    }

    fn apply(&mut self, apps: &mut [Hosted], effect: Effect) -> io::Result<()> {
        let (generation, message) = match effect {
            Effect::Prepare { generation } => {
                (generation, Some(Message::PrepareSuspend { generation }))
            }
            Effect::Resume { generation, reason } => {
                (generation, Some(Message::Resume { generation, reason }))
            }
            Effect::Enter { generation } => {
                if !self.power.entered(generation) {
                    return Ok(());
                }
                (generation, None)
            }
            Effect::Handback { .. } => {
                return Err(io::Error::other(
                    "host simulation cannot hand back to a stock reader",
                ))
            }
        };
        for app in apps.iter() {
            let tasks = {
                let mut state = app.session.state.lock().map_err(lock_error)?;
                state.power_state = self.power.state();
                state.power_generation = generation;
                if matches!(effect, Effect::Prepare { .. }) {
                    state.suspend_reply = None;
                    state.navigation.clear();
                    state.back_offer.clear();
                }
                if matches!(effect, Effect::Enter { .. }) {
                    state.hardware.frontlight_percent = 0;
                } else if let Some(percent) = self.frontlight {
                    state.hardware.frontlight_percent = percent;
                }
                state.observe_hardware();
                state.record(format!("power: {effect:?}"));
                state.tasks.clone()
            };
            if let Some(tasks) = tasks {
                let mut tasks = tasks.lock().map_err(lock_error)?;
                match effect {
                    Effect::Prepare { .. } => tasks.pause(),
                    Effect::Resume { .. } => tasks.resume(),
                    _ => {}
                }
            }
        }
        if matches!(effect, Effect::Resume { .. }) {
            self.frontlight = None;
        }
        for app in apps.iter().filter(|app| app.session.writer.connected()) {
            if let Some(message) = &message {
                crate::write_shared(
                    &app.session.writer,
                    &Frame {
                        version: kobo_protocol::VERSION,
                        request_id: 0,
                        message: message.clone(),
                    },
                )?;
            }
            if matches!(effect, Effect::Resume { .. }) {
                let mut state = app.session.state.lock().map_err(lock_error)?;
                state.update_chrome();
                state.commit_frame();
            }
        }
        Ok(())
    }
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> io::Error {
    io::Error::other("power state unavailable")
}
