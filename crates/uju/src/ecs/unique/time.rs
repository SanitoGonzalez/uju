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
}
