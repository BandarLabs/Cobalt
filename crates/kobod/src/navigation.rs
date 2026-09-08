//! Navigation policy shared by the device host and the multi-app simulator.
use std::time::Duration;

pub const BACK_GRACE_MILLIS: u64 = 2_000;
pub const MAX_HOSTED: usize = 4;

/// Prefer the oldest idle background app, then the oldest busy one. Keep the
/// launcher resident so eviction cannot remove the owner's way back.
#[must_use]
pub fn eviction<T: Copy + Ord>(
    seen: &[(u64, T, bool)],
    front: u64,
    home: Option<u64>,
) -> Option<usize> {
    seen.iter()
        .enumerate()
        .filter(|(_, (id, _, _))| *id != front && Some(*id) != home)
        .min_by_key(|(_, (_, used, busy))| (*busy, *used))
        .map(|(index, _)| index)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackRoute {
    Deliver,
    Offer,
    Leave,
}

#[must_use]
pub const fn route(is_back: bool, owns_back: bool) -> BackRoute {
    if !is_back {
        BackRoute::Deliver
    } else if owns_back {
        BackRoute::Offer
    } else {
        BackRoute::Leave
    }
}

/// One pending offer, measured in monotonic milliseconds. Repeated presses do
/// not extend the first deadline. Only that application's screen answers it.
#[derive(Clone, Debug, Default)]
pub struct BackOffer(Option<(u64, u64)>);

impl BackOffer {
    pub fn offer(&mut self, app: u64, now: u64) {
        if self.0.is_none_or(|(waiting, _)| waiting != app) {
            self.0 = Some((app, now));
        }
    }
    pub fn answer(&mut self, app: u64) {
        if self.0.is_some_and(|(waiting, _)| waiting == app) {
            self.clear();
        }
    }
    pub fn clear(&mut self) {
        self.0 = None;
    }
    #[must_use]
    pub fn remaining(&self, now: u64) -> Option<Duration> {
        self.0.map(|(_, start)| {
            Duration::from_millis(BACK_GRACE_MILLIS.saturating_sub(now.saturating_sub(start)))
        })
    }
    pub fn take_expired(&mut self, front: u64, now: u64) -> bool {
        let Some((app, start)) = self.0 else {
            return false;
        };
        if app != front {
            self.clear();
            return false;
        }
        if now.saturating_sub(start) < BACK_GRACE_MILLIS {
            return false;
        }
        self.clear();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn back_ownership_and_deadline_cannot_be_extended_by_repeated_taps() {
        assert_eq!(route(false, false), BackRoute::Deliver);
        assert_eq!(route(true, false), BackRoute::Leave);
        assert_eq!(route(true, true), BackRoute::Offer);
        let mut offer = BackOffer::default();
        offer.offer(1, 100);
        offer.offer(1, 1_000);
        offer.answer(2);
        assert!(!offer.take_expired(1, 2_099));
        assert_eq!(offer.remaining(2_099), Some(Duration::from_millis(1)));
        assert!(offer.take_expired(1, 2_100));
        assert!(!offer.take_expired(1, 2_101));
        offer.offer(1, 3_000);
        offer.answer(1);
        assert!(!offer.take_expired(1, 9_000));
        offer.offer(1, 10_000);
        assert!(!offer.take_expired(2, 12_000));
        assert_eq!(offer.remaining(12_000), None);
    }

    #[test]
    fn eviction_keeps_the_launcher_and_foreground_and_prefers_idle_work() {
        let seen = [(1, 0, false), (2, 10, true), (3, 20, false), (4, 30, false)];
        assert_eq!(eviction(&seen, 4, Some(1)), Some(2));
        assert_eq!(eviction(&seen[..2], 2, Some(1)), None);
    }
}
