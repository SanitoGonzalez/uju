use std::any::Any;

use crate::ecs::entity::Entity;

pub trait Table {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn len(&self) -> usize;
    fn remove(&mut self, entity: Entity) -> bool;
    fn contains(&self, entity: Entity) -> bool;
    fn clear(&mut self);
}
