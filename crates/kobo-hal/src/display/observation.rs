//! Bounded observations of actual submit/wait operations. No panel calibration.

use crate::refresh::{Backend, RefreshPlan};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MAX_REFRESH_OBSERVATIONS: usize = 128;

/// Correlation only, not an authentication token or physical clock calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshSession {
    process: u32,
    started_unix_nanos: u128,
    ordinal: u64,
}
impl std::fmt::Display for RefreshSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            self.process, self.started_unix_nanos, self.ordinal
        )
    }
}
impl Default for RefreshSession {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            process: std::process::id(),
            started_unix_nanos: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_nanos()),
            ordinal: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshPhase {
    Submitted,
    SubmissionFailed,
    Completed,
    CompletionFailed,
}

/// The marker and controller-specific request. A copied-back waveform is only
/// available after a successful submission; it is not proof of completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshRequest {
    pub marker: u32,
    pub backend: Backend,
    pub requested: RefreshPlan,
    pub applied: RefreshPlan,
    pub translated_waveform: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshObservation {
    pub session: RefreshSession,
    pub sequence: u64,
    /// Monotonic time from opening the display observation history.
    pub since_session_start: Duration,
    pub request: RefreshRequest,
    pub phase: RefreshPhase,
    /// Time in this submit or wait ioctl, not transport or physical settling.
    pub duration: Duration,
    /// Present for waits: time since this session recorded the submission.
    pub since_submission: Option<Duration>,
    pub errno: Option<i32>,
}
impl RefreshObservation {
    /// One bounded JSON record, with no document pixels, account data or paths.
    /// These are kernel call observations; visible ink timing still requires
    /// physical measurement. `sequence` is local to this display session.
    #[must_use]
    pub fn json(&self) -> String {
        let request = self.request;
        let phase = match self.phase {
            RefreshPhase::Submitted => "submitted",
            RefreshPhase::SubmissionFailed => "submission-failed",
            RefreshPhase::Completed => "completed",
            RefreshPhase::CompletionFailed => "completion-failed",
        };
        let backend = match request.backend {
            Backend::Hwtcon => "hwtcon",
            Backend::Mxcfb => "mxcfb-v2",
        };
        let region = request.applied.region;
        format!(
            concat!("{{\"schema\":\"cobalt.refresh-observation\",\"version\":1,",
            "\"basis\":\"device-ioctl\",\"calibrated\":false,\"sessionId\":\"{}\",\"sequence\":{},\"sessionMicros\":{},",
            "\"phase\":\"{}\",\"marker\":{},\"backend\":\"{}\",",
            "\"requestedIntent\":\"{:?}\",\"appliedIntent\":\"{:?}\",\"full\":{},",
            "\"region\":{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}},",
            "\"submittedWaveform\":{},\"translatedWaveform\":{},\"durationMicros\":{},",
            "\"sinceSubmissionMicros\":{},\"errno\":{}}}"),
            self.session,
            self.sequence,
            self.since_session_start.as_micros(),
            phase,
            request.marker,
            backend,
            request.requested.intent,
            request.applied.intent,
            request.applied.full,
            region.x,
            region.y,
            region.width,
            region.height,
            request.applied.waveform(request.backend),
            request
                .translated_waveform
                .map_or_else(|| "null".into(), |value| value.to_string()),
            self.duration.as_micros(),
            self.since_submission
                .map_or_else(|| "null".into(), |value| value.as_micros().to_string()),
            self.errno
                .map_or_else(|| "null".into(), |value| value.to_string())
        )
    }
}

#[derive(Clone, Debug)]
pub struct RefreshObservations {
    /// Oldest records no longer retained. Never imply the ring is a full run.
    pub dropped: u64,
    pub records: Vec<RefreshObservation>,
}

pub(super) struct History {
    started: Instant,
    session: RefreshSession,
    sequence: u64,
    records: VecDeque<RefreshObservation>,
}
impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("sequence", &self.sequence)
            .field("records", &self.records.len())
            .finish_non_exhaustive()
    }
}
impl Default for History {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            session: RefreshSession::default(),
            sequence: 0,
            records: VecDeque::new(),
        }
    }
}
impl History {
    pub fn record(
        &mut self,
        request: RefreshRequest,
        phase: RefreshPhase,
        duration: Duration,
        since_submission: Option<Duration>,
        errno: Option<i32>,
    ) {
        static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        self.sequence = self.sequence.saturating_add(1);
        let observation = RefreshObservation {
            session: self.session,
            sequence: self.sequence,
            since_session_start: self.started.elapsed(),
            request,
            phase,
            duration,
            since_submission,
            errno,
        };
        if self.records.len() == MAX_REFRESH_OBSERVATIONS {
            self.records.pop_front();
        }
        self.records.push_back(observation);
        if *LOG.get_or_init(|| std::env::var("KOBO_FRAME_TIMING").ok().as_deref() == Some("1")) {
            eprintln!("{}", observation.json());
        }
    }
    pub fn snapshot(&self) -> RefreshObservations {
        RefreshObservations {
            dropped: self.sequence.saturating_sub(self.records.len() as u64),
            records: self.records.iter().copied().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refresh::{Rect, RefreshIntent};
    fn request(marker: u32) -> RefreshRequest {
        let requested = RefreshPlan::new(
            Rect {
                x: 1,
                y: 2,
                width: 32,
                height: 32,
            },
            RefreshIntent::ColourContent,
            false,
            1072,
            1448,
        )
        .unwrap();
        RefreshRequest {
            marker,
            backend: Backend::Hwtcon,
            requested,
            applied: RefreshPlan {
                intent: RefreshIntent::QualityContent,
                ..requested
            },
            translated_waveform: Some(2),
        }
    }
    #[test]
    fn record_distinguishes_request_translation_submission_and_completion() {
        let mut history = History::default();
        history.record(
            request(123),
            RefreshPhase::Submitted,
            Duration::from_micros(7),
            None,
            None,
        );
        history.record(
            request(123),
            RefreshPhase::CompletionFailed,
            Duration::from_micros(11),
            Some(Duration::from_micros(20)),
            Some(5),
        );
        let snapshot = history.snapshot();
        assert_eq!(snapshot.records.len(), 2);
        assert_eq!(
            snapshot.records[0].request.marker,
            snapshot.records[1].request.marker
        );
        assert!(snapshot.records[0]
            .json()
            .contains("\"sinceSubmissionMicros\":null"));
        let json = snapshot.records[1].json();
        for fragment in [
            "\"phase\":\"completion-failed\"",
            "\"requestedIntent\":\"ColourContent\"",
            "\"appliedIntent\":\"QualityContent\"",
            "\"errno\":5",
            "\"calibrated\":false",
        ] {
            assert!(json.contains(fragment), "{json}");
        }
        assert_eq!(
            history.snapshot().records,
            snapshot.records,
            "reading observations has no panel side effects"
        );
    }
    #[test]
    fn ring_explicitly_reports_dropped_records() {
        let mut history = History::default();
        for marker in 1..=200 {
            history.record(
                request(marker),
                RefreshPhase::Submitted,
                Duration::ZERO,
                None,
                None,
            );
        }
        let snapshot = history.snapshot();
        assert_eq!(snapshot.records.len(), MAX_REFRESH_OBSERVATIONS);
        assert_eq!(snapshot.dropped, 72);
        assert_eq!(snapshot.records[0].sequence, 73);
        assert_eq!(snapshot.records.last().unwrap().sequence, 200);
    }
}
