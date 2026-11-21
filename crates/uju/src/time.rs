use std::time::Duration;

use crate::ecs::Unique;

#[derive(Unique, Default)]
pub struct Time {
    ticks: u64,
    delta: Duration,
}

impl Time {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    pub fn delta(&self) -> Duration {
        self.delta
    }

    /// Counts one more tick, which took `delta`.
    #[cfg_attr(not(test), expect(dead_code, reason = "called by the runtime's tick"))]
    pub(crate) fn advance(&mut self, delta: Duration) {
        self.ticks += 1;
        self.delta = delta;
    }
}

/// A count of ticks left, which [`tick`](Self::tick) counts down.
///
/// It stores what is left rather than an end tick, since shards' tick
/// counters are unrelated and an entity carries its timers across them.
/// The default timer is finished.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct Timer(u32);

impl Timer {
    /// A timer that finishes after `ticks` calls to [`tick`](Self::tick).
    /// `Timer::new(0)` is finished already.
    pub const fn new(ticks: u32) -> Self {
        Self(ticks)
    }

    /// The ticks left.
    pub const fn remaining(self) -> u32 {
        self.0
    }

    pub const fn is_finished(self) -> bool {
        self.0 == 0
    }

    /// Counts one tick down, and returns `true` on the tick the timer
    /// reaches zero. A finished timer stays at zero and returns `false`, so
    /// `true` comes once per start.
    pub fn tick(&mut self) -> bool {
        match self.0 {
            0 => false,
            left => {
                self.0 = left - 1;
                self.0 == 0
            }
        }
    }
}

impl From<u8> for Timer {
    fn from(ticks: u8) -> Self {
        Self(ticks.into())
    }
}

impl From<u16> for Timer {
    fn from(ticks: u16) -> Self {
        Self(ticks.into())
    }
}

impl From<u32> for Timer {
    fn from(ticks: u32) -> Self {
        Self(ticks)
    }
}

impl From<Timer> for u32 {
    fn from(timer: Timer) -> Self {
        timer.0
    }
}

impl From<Timer> for u64 {
    fn from(timer: Timer) -> Self {
        timer.0.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_counts_down_once() {
        let mut timer = Timer::new(3);
        assert!(!timer.is_finished());
        assert!(!timer.tick());
        assert!(!timer.tick());
        assert_eq!(timer.remaining(), 1);
        assert!(timer.tick());
        assert!(timer.is_finished());

        // finished: stays at zero, never fires again
        assert!(!timer.tick());
        assert!(!timer.tick());
        assert_eq!(timer.remaining(), 0);

        // restarted
        timer = Timer::new(1);
        assert!(timer.tick());
    }

    #[test]
    fn timer_new_zero_is_finished() {
        let mut timer = Timer::new(0);
        assert!(timer.is_finished());
        assert!(!timer.tick());
        assert_eq!(timer, Timer::default());
    }

    #[test]
    fn timer_converts() {
        assert_eq!(Timer::from(7u8), Timer::new(7));
        assert_eq!(Timer::from(u16::MAX).remaining(), 65535);
        assert_eq!(u32::from(Timer::from(u32::MAX)), u32::MAX);
        assert_eq!(u64::from(Timer::new(9)), 9);
    }
}
