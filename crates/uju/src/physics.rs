use derive_more::From;

use crate::ecs::Component;
use crate::math::Vector;

/// A unit-per-second vector
#[derive(Component, Clone, Copy, Debug, From, PartialEq)]
#[repr(transparent)]
pub struct Velocity(Vector);
