use derive_more::From;
use glam::Vec2;

/// A unit-per-second-squared 2D vector
#[derive(Clone, Copy, Debug, From, PartialEq)]
#[repr(transparent)]
pub struct Velocity2(Vec2);
