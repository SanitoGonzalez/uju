use crate::ecs::Component;
use crate::ecs::storage::tracked::Tracked;
use crate::math::{Rotation, Vector};

/// The position and rotation of an entity
///
/// With the `3d-yaw` feature, `rotation` is a yaw around the Y axis. Its
/// table tracks writes, so uju sets the cells of the entities that moved.
#[derive(Component, Clone, Debug)]
#[component(storage = Tracked<Self>)]
pub struct Transform {
    pub position: Vector,
    pub rotation: Rotation,
}
