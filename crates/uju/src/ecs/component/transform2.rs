use glam::Vec2;

use crate::ecs::Component;
use crate::math::Radians;

#[derive(Component, Clone, Debug)]
pub struct Transform2 {
    pub position: Vec2,
    pub rotation: Radians,
}
