//! Hold classification shared by device input and simulator replay.
use crate::TouchEvent;

/// Existing runtime hold thresholds. Physical calibration is separate from
/// exercising the same policy in a simulator.
pub const HOLD_MILLIS: u64 = 500;
pub const HOLD_SLIP_PIXELS: u32 = 40;

#[derive(Clone, Debug, Default)]
pub struct HoldTracker {
    landed: Option<(u64, u32, u32)>,
}
impl HoldTracker {
    /// Returns true only on a release after an uninterrupted, stationary hold.
    /// Time is supplied by the caller's monotonic clock. A regressing clock
    /// cannot manufacture a hold; cancel and excessive movement clear it.
    pub fn observe(&mut self, event: TouchEvent, monotonic_millis: u64) -> bool {
        match event {
            TouchEvent::Down { x, y } => self.landed = Some((monotonic_millis, x, y)),
            TouchEvent::Move { x, y } => {
                if self.landed.is_some_and(|(_, from_x, from_y)| {
                    x.abs_diff(from_x) > HOLD_SLIP_PIXELS || y.abs_diff(from_y) > HOLD_SLIP_PIXELS
                }) {
                    self.landed = None;
                }
            }
            TouchEvent::Cancel => self.landed = None,
            TouchEvent::Up { .. } => {
                return self.landed.take().is_some_and(|(at, _, _)| {
                    monotonic_millis
                        .checked_sub(at)
                        .is_some_and(|elapsed| elapsed >= HOLD_MILLIS)
                })
            }
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn holds_require_time_and_stationary_uninterrupted_contact() {
        let mut tracker = HoldTracker::default();
        let down = TouchEvent::Down { x: 100, y: 100 };
        let up = TouchEvent::Up { x: 100, y: 100 };
        assert!(!tracker.observe(up, 500));
        tracker.observe(down, 0);
        assert!(!tracker.observe(up, 499));
        tracker.observe(down, 1000);
        assert!(tracker.observe(up, 1500));
        assert!(!tracker.observe(up, 2000));
        tracker.observe(down, 2000);
        tracker.observe(TouchEvent::Move { x: 141, y: 100 }, 2100);
        tracker.observe(TouchEvent::Move { x: 100, y: 100 }, 2300);
        assert!(!tracker.observe(up, 2600));
        tracker.observe(down, 3000);
        tracker.observe(TouchEvent::Cancel, 3100);
        assert!(!tracker.observe(up, 4000));
        tracker.observe(down, 5000);
        assert!(!tracker.observe(up, 4000));
    }
}
