use std::any::Any;
use std::fmt;

use crate::ecs::component::{Component, Replication};
use crate::ecs::entity::{Entity, Uid};
use crate::ecs::storage::Storage;
use crate::net::wire::{DecodeError, EncodeError};

/// A component's table without its type, as the `World` and the runtime
/// drive it.
///
/// The replication methods mean something only for a table whose
/// [`replication`](Table::replication) is `Some`. The others write nothing,
/// report no change, and panic when asked to apply a section.
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

    /// Drops the values of despawned entities. Unlike a removal, a despawn
    /// is not in the remove log, since the frame's `gone` covers it. The
    /// component's `on_remove` hooks still get the values.
    fn despawn_batch(&mut self, entities: &[Entity]) {
        self.remove_batch(entities);
    }

    fn contains(&self, entity: Entity) -> bool;
    fn clear(&mut self);

    /// Formats `entity`'s value with [`Component::inspect`], for
    /// [`World::inspect`](crate::ecs::world::World::inspect). Writes nothing
    /// when it holds none.
    fn inspect(&self, entity: Entity, f: &mut fmt::Formatter<'_>) -> fmt::Result;

    /// Clears the tick's changes: dirty and added bits, and the remove log.
    fn end_tick(&mut self) {}

    /// How the component travels, `None` for one that does not.
    fn replication(&self) -> Option<Replication> {
        None
    }

    /// Appends the entities whose value was added or changed this tick, in
    /// dense order.
    fn changed(&self, _out: &mut Vec<Entity>) {}

    /// The entities that lost the component this tick, in removal order.
    /// Some may be dead, or hold the component again.
    fn removed(&self) -> &[Entity] {
        &[]
    }

    /// Appends one section of a cell's sub-frame: the full values of the
    /// `full` entities, such as those that entered the cell, the changes of
    /// the `changed` ones, and the `removed` uids.
    ///
    /// Entities that hold no value are skipped, and an entity must not be in
    /// both lists. A snapshot is a section with only `full`. Returns whether
    /// it wrote a section, which it does not when nothing remains to write.
    fn encode_section(
        &self,
        _full: &[(Uid, Entity)],
        _changed: &[(Uid, Entity)],
        _removed: &[Uid],
        _out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        Ok(false)
    }

    /// Applies the section at the start of `buf`, which begins after the
    /// component id, and returns its length. `resolve` maps each uid to the
    /// entity it lands on, or to `None` to skip the item.
    ///
    /// Received state is not a change of this world: it marks no bit and
    /// logs nothing in the tick's changes. The component's hooks still see
    /// the values gained and removed, but not a value overwritten.
    fn apply_section(
        &mut self,
        _buf: &[u8],
        _resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        panic!("this component is not replicated")
    }
}

/// [`Table::inspect`] for a storage.
pub(crate) fn inspect<S: Storage>(
    storage: &S,
    entity: Entity,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    match storage.get(entity) {
        Some(value) => Component::inspect(value, f),
        None => Ok(()),
    }
}
