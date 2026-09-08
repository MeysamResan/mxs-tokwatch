//! Short, reversible transitions. No timer is needed once the target is reached.
use std::time::{Duration, Instant};
#[derive(Clone, Copy)]
pub(super) struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}
impl Tween {
    pub(super) fn new(from: f32, to: f32, now: Instant, millis: u64) -> Self {
        Self {
            from,
            to,
            start: now,
            duration: Duration::from_millis(millis),
        }
    }
    pub(super) fn sample(self, now: Instant) -> (f32, bool) {
        let t = (now.saturating_duration_since(self.start).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = if self.to >= self.from {
            1.0 - (1.0 - t).powi(3)
        } else {
            t * t
        };
        (self.from + (self.to - self.from) * eased, t >= 1.0)
    }
    pub(super) fn target(self) -> f32 {
        self.to
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transitions_are_bounded_monotonic_and_finish_exactly() {
        let now = Instant::now();
        for (from, to) in [(0.0, 1.0), (1.0, 0.0), (0.3, 1.0)] {
            let tween = Tween::new(from, to, now, 180);
            let mut previous = from;
            for ms in 0..=200 {
                let (value, done) = tween.sample(now + Duration::from_millis(ms));
                assert!((0.0..=1.0).contains(&value));
                assert!(if to >= from {
                    value >= previous
                } else {
                    value <= previous
                });
                assert_eq!(done, ms >= 180);
                previous = value;
            }
            assert_eq!(previous, to);
        }
    }
    #[test]
    fn a_reversed_transition_starts_at_the_current_position() {
        let now = Instant::now();
        let first = Tween::new(0.0, 1.0, now, 180);
        let halfway = now + Duration::from_millis(70);
        let value = first.sample(halfway).0;
        let reversed = Tween::new(value, 0.0, halfway, 100);
        assert_eq!(reversed.sample(halfway).0, value);
        assert_eq!(
            reversed.sample(halfway + Duration::from_millis(100)),
            (0.0, true)
        );
    }
}
