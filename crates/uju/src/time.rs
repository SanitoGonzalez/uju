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
