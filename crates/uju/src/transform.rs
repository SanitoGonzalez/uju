use crate::ecs::Component;
use crate::math::{Rotation, Vector};

/// The position and rotation of an entity
///
/// With the `3d-yaw` feature, `rotation` is a yaw around the Y axis.
#[derive(Component, Clone, Debug)]
pub struct Transform {
    pub position: Vector,
    pub rotation: Rotation,
}
