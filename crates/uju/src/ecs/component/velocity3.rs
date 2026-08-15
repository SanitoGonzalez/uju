use derive_more::From;
use glam::Vec3A;

/// A unit-per-second-squared 2D vector
#[derive(Clone, Copy, Debug, From, PartialEq)]
#[repr(transparent)]
pub struct Velocity3(Vec3A);
