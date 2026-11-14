use crate::ecs::Component;
use crate::math::{Rotation, Vector};

/// The position and rotation of an entity
#[derive(Component, Clone, Debug)]
pub struct Transform {
    pub position: Vector,
    pub rotation: Rotation,
}
