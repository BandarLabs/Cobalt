use super::{
    frame::{FramePlanner, FrameTransition, PanelWaveform},
    PROFILE,
};
use kobo_ui::Surface;
use std::{io, time::Instant};

/// The panel's visible state, including a deliberately labelled approximation
/// of residue left by non-cleaning updates.
#[derive(Debug)]
pub(super) struct PanelPreview {
    pub planner: FramePlanner,
    ideal: Vec<u8>,
    visible: Vec<u8>,
    pub last: Option<FrameTransition>,
    desired: Option<Surface>,
    pending: Option<Pending>,
    held: bool,
    failed: bool,
    known: bool,
    submitted: u64,
    completed: u64,
    failures: u64,
    started: Instant,
    submitted_at: Option<u64>,
    finished_at: Option<u64>,
}

impl PanelPreview {
    pub fn new() -> Self {
        let width = PROFILE.width as usize;
        let height = PROFILE.height as usize;
        let pixels = width.saturating_mul(height);
        Self {
            planner: FramePlanner::new(width, height),
            ideal: vec![kobo_ui::tone::PAPER; pixels],
            visible: vec![kobo_ui::tone::INK; pixels],
            last: None,
            desired: None,
            pending: None,
            held: false,
            failed: false,
            known: false,
            submitted: 0,
            completed: 0,
            failures: 0,
            started: Instant::now(),
            submitted_at: None,
            finished_at: None,
        }
    }

    pub fn update(&mut self, surface: &Surface) {
        self.ideal.clone_from(&surface.pixels);
        self.desired = Some(surface.clone());
        if self.pending.is_none() && !self.failed {
            self.submit_latest();
        }
    }

    fn millis(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn submit_latest(&mut self) {
        let Some(surface) = &self.desired else {
            return;
        };
        let Some(transition) = self.planner.plan(surface) else {
            self.last = None;
            return;
        };
        self.pending = Some(Pending {
            surface: surface.clone(),
            transition,
        });
        self.submitted = self.submitted.saturating_add(1);
        self.submitted_at = Some(self.millis());
        self.finished_at = None;
        if !self.held {
            self.complete_pending();
        }
    }

    fn complete_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if !self.planner.commit(&pending.surface, &pending.transition) {
            self.planner.invalidate();
            self.failed = true;
            self.known = false;
            self.failures = self.failures.saturating_add(1);
            return;
        }
        if pending.transition.full {
            self.visible.clone_from(&pending.surface.pixels);
        } else {
            self.apply_partial(&pending.surface, &pending.transition);
        }
        self.known = true;
        self.completed = self.completed.saturating_add(1);
        self.finished_at = Some(self.millis());
        self.last = Some(pending.transition);
        // Retain only the newest requested frame while busy. It is planned
        // against the confirmed completed frame, never against a pending one.
        if self
            .desired
            .as_ref()
            .is_some_and(|desired| desired != &pending.surface)
        {
            self.submit_latest();
        }
    }

    pub fn accepts_input(&self) -> bool {
        self.known && self.pending.is_none() && !self.failed
    }

    pub fn control(&mut self, command: &str) -> io::Result<()> {
        match command.trim() {
            "hold" => self.held = true,
            "auto" if self.pending.is_none() && !self.failed => self.held = false,
            "complete" if self.pending.is_some() => self.complete_pending(),
            "fail" if self.pending.take().is_some() => {
                self.failed = true;
                self.known = false;
                self.failures = self.failures.saturating_add(1);
                self.finished_at = Some(self.millis());
                self.planner.invalidate();
            },
            "retry" if self.failed => { self.failed = false; self.submit_latest(); },
            _ => return Err(io::Error::other("panel expects hold, auto (when idle), complete or fail (when busy), or retry (after failure)")),
        }
        Ok(())
    }

    pub fn state_json(&self) -> kobo_json::Value {
        let stamp = |value: Option<u64>| {
            value.map_or(kobo_json::Value::Null, |value| {
                kobo_json::Value::from(value.to_string())
            })
        };
        kobo_json::ObjectBuilder::new()
            .set("mode", if self.held { "held" } else { "automatic" })
            .set(
                "status",
                if self.failed {
                    "failed"
                } else if self.pending.is_some() {
                    "busy"
                } else {
                    "idle"
                },
            )
            .set("contentsKnown", self.known && self.pending.is_none())
            .set(
                "visibleFrame",
                "last confirmed completion with approximate residue",
            )
            .set(
                "queued",
                self.pending.as_ref().is_some_and(|pending| {
                    self.desired
                        .as_ref()
                        .is_some_and(|desired| desired != &pending.surface)
                }),
            )
            .set("submitted", self.submitted.to_string())
            .set("completed", self.completed.to_string())
            .set("failures", self.failures.to_string())
            .set(
                "submissionMarker",
                self.pending.as_ref().map_or(kobo_json::Value::Null, |_| {
                    kobo_json::Value::from(self.submitted.to_string())
                }),
            )
            .set("submittedAtMillis", stamp(self.submitted_at))
            .set("finishedAtMillis", stamp(self.finished_at))
            .set(
                "timing",
                "host control observations; not hardware calibration",
            )
            .build()
    }

    fn apply_partial(&mut self, surface: &Surface, transition: &FrameTransition) {
        for update in &transition.regions {
            let (Ok(left), Ok(top), Ok(width), Ok(height)) = (
                usize::try_from(update.region.x),
                usize::try_from(update.region.y),
                usize::try_from(update.region.width),
                usize::try_from(update.region.height),
            ) else {
                continue;
            };
            for y in top..top.saturating_add(height) {
                let row = y.saturating_mul(surface.width);
                for x in left..left.saturating_add(width) {
                    let index = row.saturating_add(x);
                    let Some(target) = surface.pixels.get(index).copied() else {
                        continue;
                    };
                    let Some(visible) = self.visible.get_mut(index) else {
                        continue;
                    };
                    let target = match update.waveform {
                        PanelWaveform::Du => {
                            if target < 128 {
                                kobo_ui::tone::INK
                            } else {
                                kobo_ui::tone::PAPER
                            }
                        }
                        // The simulated panel is a Clara BW, which has no colour
                        // filter: a colour update lands as its luminance, exactly
                        // as the runtime writes it on that device.
                        PanelWaveform::Gl16 | PanelWaveform::Gc16 | PanelWaveform::Colour => target,
                    };
                    // An LCD cannot reproduce electrophoretic residue. Retaining
                    // one sixteenth of the previous displayed value makes stale
                    // edges visible without claiming hardware-measured physics.
                    *visible = u8::try_from((u16::from(target) * 15 + u16::from(*visible)) / 16)
                        .unwrap_or(target);
                }
            }
        }
    }

    pub fn frame(&self, ideal: bool) -> &[u8] {
        if ideal {
            &self.ideal
        } else {
            &self.visible
        }
    }
}

#[derive(Debug)]
struct Pending {
    surface: Surface,
    transition: FrameTransition,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_frames_coalesce_without_committing_until_completion_and_failed_retry_cleans() {
        let mut panel = PanelPreview::new();
        let mut surface = Surface::new(PROFILE.width as usize, PROFILE.height as usize);
        panel.update(&surface);
        assert_eq!(panel.planner.refreshes(), 1);
        assert!(panel.accepts_input());
        panel.control("hold").unwrap();
        surface.pixels[10] = 0;
        panel.update(&surface);
        let before = panel.state_json();
        let visible = panel.frame(false).to_vec();
        for _ in 0..8 {
            panel.frame(false);
            panel.frame(true);
            assert_eq!(panel.state_json(), before);
        }
        assert!(!panel.accepts_input());
        assert_eq!(panel.planner.refreshes(), 1);
        surface.pixels[20] = 100;
        panel.update(&surface);
        assert_eq!(panel.frame(false), visible);
        assert_eq!(panel.pending.as_ref().unwrap().surface.pixels[20], 255);
        assert_eq!(panel.desired.as_ref().unwrap().pixels[20], 100);
        panel.control("complete").unwrap();
        assert_eq!(panel.planner.refreshes(), 2);
        assert!(panel.pending.is_some());
        panel.control("complete").unwrap();
        assert_eq!(panel.planner.refreshes(), 3);
        assert!(panel.accepts_input());
        let complete = panel.state_json();
        assert!(panel.control("complete").is_err());
        assert_eq!(panel.state_json(), complete);
        surface.pixels[30] = 80;
        panel.update(&surface);
        let confirmed = panel.frame(false).to_vec();
        panel.control("fail").unwrap();
        assert_eq!(panel.planner.refreshes(), 3);
        assert_eq!(panel.frame(false), confirmed);
        assert!(!panel.accepts_input());
        panel.control("retry").unwrap();
        assert!(panel.pending.as_ref().unwrap().transition.full);
        assert!(!panel.known);
        panel.control("complete").unwrap();
        assert_eq!(panel.frame(false), surface.pixels);
        assert_eq!(panel.planner.refreshes(), 4);
        assert!(panel.accepts_input());
        assert_eq!(panel.failures, 1);
        panel.control("auto").unwrap();
    }
}
