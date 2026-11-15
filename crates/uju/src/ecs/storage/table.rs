use std::any::Any;

use crate::ecs::entity::Entity;

pub trait Table {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn remove(&mut self, entity: Entity) -> bool;

    /// Removes each entity's value. A dead or absent entity is a probe that
    /// misses.
    fn remove_batch(&mut self, entities: &[Entity]) {
        for &entity in entities {
            self.remove(entity);
        }
    }

    fn contains(&self, entity: Entity) -> bool;
    fn clear(&mut self);
}
