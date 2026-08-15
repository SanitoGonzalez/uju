use glam::{Vec3, Quat};

use crate::ecs::Component;

#[derive(Component, Clone, Debug)]
pub struct Transform3 {
    pub position: Vec3,
    pub rotation: Quat,
}
