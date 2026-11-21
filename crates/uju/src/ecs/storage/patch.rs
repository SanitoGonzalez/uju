use std::any::Any;
use std::cell::Cell;
use std::marker::PhantomData;

use crate::ecs::component::{Patch, Replication};
use crate::ecs::entity::{Entity, Uid};
use crate::ecs::hook::HookLog;
use crate::ecs::storage::{
    Storage, Tracking,
    bits::Marker,
    join::{Entities, Joinable},
    section::{self, CHANGES, REMOVED, VALUES},
    sparse_set::SparseSet,
    table::Table,
    tracked::Tracked,
};
use crate::net::wire::{Decode, DecodeError, EncodeError};

/// The table of a `patch` component: a [`Tracked`] table with a field mask
/// per entry, beside the values.
///
/// Mutable access yields the component's [`Patch::Mut`] wrapper, whose
/// `*_mut()` accessors mark their field in the mask and the entry dirty.
/// `insert` over a value marks every field. An entry added this tick is
/// sent whole, and a dirty one as a patch of its marked fields. The masks
/// are cleared at the end of the tick, with the bits.
///
/// Like a [`ReplaceTable`](crate::ecs::storage::replace::ReplaceTable), it
/// skips replicas on mutable access through a query or a view, and applies
/// received state untracked.
pub struct PatchTable<T: Patch> {
    tracked: Tracked<T>,
    /// [`Self::MASK`] bytes per entry, in dense order.
    masks: Vec<u8>,
}

impl<T: Patch> PatchTable<T> {
    /// The bytes of one entry's mask.
    const MASK: usize = T::FIELDS.div_ceil(8);

    pub fn new() -> Self {
        Self {
            tracked: Tracked::new(),
            masks: Vec::new(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.tracked.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tracked.is_empty()
    }

    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        self.tracked.contains(entity)
    }

    #[inline]
    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.tracked.get(entity)
    }

    /// `entity`'s value, in the wrapper that marks the fields written
    /// through it. It does not skip replicas: a
    /// [`ViewMut`](crate::ecs::storage::view::ViewMut)'s `get_mut` does.
    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Option<T::Mut<'_>> {
        let position = self.tracked.index_of(entity)?;
        let (set, dirty) = self.tracked.split_mut();
        let masks = Cell::from_mut(self.masks.as_mut_slice()).as_slice_of_cells();
        Some(T::new_mut(
            set.value_mut_at(position),
            FieldMask::new(masks, dirty, position),
        ))
    }

    #[inline]
    pub fn entities(&self) -> &[Entity] {
        self.tracked.entities()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.tracked.iter()
    }

    pub fn components(&self) -> impl Iterator<Item = &T> {
        self.tracked.components()
    }

    /// Inserts or replaces `entity`'s value, returning the previous one
    /// unless an `on_remove` hook keeps it. A replaced value has every field
    /// marked, and a new one is added.
    pub fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        let (replaced, previous) = self.tracked.insert_replacing(entity, component);
        match replaced {
            false => self.masks.resize(self.len() * Self::MASK, 0),
            true => {
                let position = self.tracked.index_of(entity).unwrap();
                for (index, byte) in self.mask_at_mut(position).iter_mut().enumerate() {
                    *byte = full::<T>(index);
                }
            }
        }
        previous
    }

    /// See [`Tracked::remove`].
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        self.remove_with(entity, Tracked::remove).flatten()
    }

    #[inline]
    pub fn is_changed(&self, entity: Entity) -> bool {
        self.tracked.is_changed(entity)
    }

    #[inline]
    pub fn is_added(&self, entity: Entity) -> bool {
        self.tracked.is_added(entity)
    }

    pub fn removed(&self) -> &[Entity] {
        self.tracked.removed()
    }

    pub fn changed(&self) -> impl Iterator<Item = Entity> + '_ {
        self.tracked.changed()
    }

    /// The fields of `entity`'s value marked this tick, a bit per field in
    /// declaration order.
    pub fn mask(&self, entity: Entity) -> Option<&[u8]> {
        Some(self.mask_at(self.tracked.index_of(entity)?))
    }

    pub fn clear(&mut self) {
        self.tracked.clear();
        self.masks.clear();
    }

    #[inline]
    fn mask_at(&self, position: usize) -> &[u8] {
        &self.masks[position * Self::MASK..][..Self::MASK]
    }

    #[inline]
    fn mask_at_mut(&mut self, position: usize) -> &mut [u8] {
        &mut self.masks[position * Self::MASK..][..Self::MASK]
    }

    fn insert_untracked(&mut self, entity: Entity, component: T) {
        if self.tracked.insert_untracked(entity, component).is_none() {
            self.masks.resize(self.len() * Self::MASK, 0);
        }
    }

    /// Removes `entity`'s value with `remove`, and moves the last entry's
    /// mask into the hole, as the value moves. `None` when `entity` holds
    /// no value, else what `remove` returned.
    fn remove_with(
        &mut self,
        entity: Entity,
        remove: fn(&mut Tracked<T>, Entity) -> Option<T>,
    ) -> Option<Option<T>> {
        let position = self.tracked.index_of(entity)?;
        let last = self.len() - 1;
        let value = remove(&mut self.tracked, entity);
        self.masks.copy_within(
            last * Self::MASK..(last + 1) * Self::MASK,
            position * Self::MASK,
        );
        self.masks.truncate(last * Self::MASK);
        Some(value)
    }

    fn end_tick(&mut self) {
        // a mask holds bits only where the dirty bit is set
        for position in self.tracked.changed_positions() {
            self.masks[position * Self::MASK..][..Self::MASK].fill(0);
        }
        self.tracked.end_tick();
    }

    fn write_section(
        &self,
        full: &[(Uid, Entity)],
        changed: &[(Uid, Entity)],
        removed: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        // an entry added this tick is sent whole, a dirty one as a patch
        let positions = |added: bool| {
            changed.iter().filter_map(move |&(uid, entity)| {
                let position = self.tracked.index_of(entity)?;
                (self.tracked.is_added_at(position) == added).then_some((uid, position))
            })
        };
        section::encode::<T>(out, |parts| {
            parts.part(VALUES, |out| {
                for &(uid, entity) in full {
                    if let Some(value) = self.get(entity) {
                        section::put_value(out, uid, value)?;
                    }
                }
                for (uid, position) in positions(true) {
                    section::put_value(out, uid, self.tracked.value_at(position))?;
                }
                Ok(())
            })?;
            parts.part(CHANGES, |out| {
                let width = section::prefix_width(T::MAX_PATCH_SIZE);
                for (uid, position) in positions(false) {
                    let (value, mask) = (self.tracked.value_at(position), self.mask_at(position));
                    section::put_sized(out, uid, width, value.patch_size(mask), |buf| {
                        value.encode_patch(mask, buf)
                    })?;
                }
                Ok(())
            })?;
            parts.removed(removed)
        })
    }

    fn apply(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        section::decode::<T>(
            buf,
            VALUES | CHANGES | REMOVED,
            |part, items, at| match part {
                VALUES => section::read_values::<T>(items, at, |uid, value| {
                    if let Some(entity) = resolve(uid) {
                        self.insert_untracked(entity, value);
                    }
                }),
                CHANGES => {
                    let width = section::prefix_width(T::MAX_PATCH_SIZE);
                    section::read_sized::<T>(items, at, None, width, |uid, bytes| {
                        let patch = T::Patch::decode(bytes)?;
                        // a patch to a value the receiver does not hold is skipped
                        if let Some(value) =
                            resolve(uid).and_then(|e| self.tracked.get_mut_untracked(e))
                        {
                            value.apply(patch);
                        }
                        Ok(())
                    })
                }
                _ => section::read_uids::<T>(items, at, |uid| {
                    if let Some(entity) = resolve(uid) {
                        self.remove_with(entity, Tracked::remove_untracked);
                    }
                }),
            },
        )
    }
}

/// The byte `index` of a mask with every field set.
#[inline]
fn full<T: Patch>(index: usize) -> u8 {
    match T::FIELDS - index * 8 {
        8.. => u8::MAX,
        rest => (1 << rest) - 1,
    }
}

impl<T: Patch> Default for PatchTable<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// One entry's field mask, which a [`Patch::Mut`] wrapper marks when it
/// hands out a field mutably. Marking a field also marks the entry dirty.
///
/// The wrappers alive at once share the table's masks and dirty bits, and
/// set their bits through `Cell`s.
pub struct FieldMask<'a, T> {
    mask: &'a [Cell<u8>],
    dirty: Marker<'a>,
    position: usize,
    component: PhantomData<fn() -> T>,
}

impl<'a, T: Patch> FieldMask<'a, T> {
    #[inline]
    fn new(masks: &'a [Cell<u8>], dirty: Marker<'a>, position: usize) -> Self {
        let mask = PatchTable::<T>::MASK;
        Self {
            mask: &masks[position * mask..][..mask],
            dirty,
            position,
            component: PhantomData,
        }
    }

    /// Marks field `field`, counted from `0` in declaration order.
    #[inline]
    pub fn mark(&self, field: usize) {
        debug_assert!(field < T::FIELDS);
        let byte = &self.mask[field / 8];
        byte.set(byte.get() | 1 << (field % 8));
        self.dirty.set(self.position);
    }

    /// Marks every field.
    #[inline]
    pub fn mark_all(&self) {
        for (index, byte) in self.mask.iter().enumerate() {
            byte.set(full::<T>(index));
        }
        self.dirty.set(self.position);
    }
}

/// [`PatchTable`]'s mutable join term. Its items share the masks and the
/// dirty bits.
pub struct PatchJoin<'a, T: Patch> {
    set: &'a mut SparseSet<T>,
    dirty: Marker<'a>,
    masks: &'a [Cell<u8>],
}

// SAFETY: delegates to the `&mut SparseSet` term's guarantees: `entities`
// yields the dense entities once each, and `get` probes the set once per
// entity.
unsafe impl<'a, T: Patch> Joinable<'a> for PatchJoin<'a, T> {
    type Item = T::Mut<'a>;

    #[inline]
    fn len(&self) -> usize {
        self.set.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        unsafe { self.set.entities_unbound() }
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        let (position, value) = unsafe { self.set.get_mut_indexed_unbound(entity)? };
        Some(T::new_mut(
            value,
            FieldMask::new(self.masks, self.dirty, position),
        ))
    }
}

impl<T: Patch> Table for PatchTable<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn remove(&mut self, entity: Entity) -> bool {
        self.remove_with(entity, Tracked::remove).is_some()
    }

    fn despawn_batch(&mut self, entities: &[Entity]) {
        for &entity in entities {
            self.remove_with(entity, Tracked::remove_untracked);
        }
    }

    #[inline]
    fn contains(&self, entity: Entity) -> bool {
        self.contains(entity)
    }

    fn clear(&mut self) {
        self.clear();
    }

    fn end_tick(&mut self) {
        self.end_tick();
    }

    fn replication(&self) -> Option<Replication> {
        Some(Replication::of::<T>())
    }

    fn changed(&self, out: &mut Vec<Entity>) {
        out.extend(self.tracked.changed());
    }

    fn removed(&self) -> &[Entity] {
        self.tracked.removed()
    }

    fn encode_section(
        &self,
        full: &[(Uid, Entity)],
        changed: &[(Uid, Entity)],
        removed: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        self.write_section(full, changed, removed, out)
    }

    fn apply_section(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        self.apply(buf, resolve)
    }
}

impl<T: Patch> Storage for PatchTable<T> {
    type Component = T;
    type Mut<'a> = T::Mut<'a>;
    type JoinMut<'a> = PatchJoin<'a, T>;

    const REPLICATED: bool = true;

    #[inline]
    fn entities(&self) -> &[Entity] {
        self.entities()
    }

    #[inline]
    fn get(&self, entity: Entity) -> Option<&T> {
        self.get(entity)
    }

    #[inline]
    fn get_mut(&mut self, entity: Entity) -> Option<T::Mut<'_>> {
        self.get_mut(entity)
    }

    fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        self.insert(entity, component)
    }

    fn remove(&mut self, entity: Entity) -> Option<T> {
        self.remove(entity)
    }

    #[inline]
    fn hook_log(&mut self) -> &mut HookLog<T> {
        self.tracked.hook_log()
    }

    #[inline]
    fn join_mut(&mut self) -> PatchJoin<'_, T> {
        let (set, dirty) = self.tracked.split_mut();
        PatchJoin {
            set,
            dirty,
            masks: Cell::from_mut(self.masks.as_mut_slice()).as_slice_of_cells(),
        }
    }
}

impl<T: Patch> Tracking for PatchTable<T> {
    #[inline]
    fn is_changed(&self, entity: Entity) -> bool {
        self.is_changed(entity)
    }

    #[inline]
    fn is_added(&self, entity: Entity) -> bool {
        self.is_added(entity)
    }

    fn removed(&self) -> &[Entity] {
        self.removed()
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use super::*;
    use crate::ecs::Component;
    use crate::ecs::component::{Component as _, Replicated, Valued, Visibility};
    use crate::ecs::query::{Changed, Mut, Query};
    use crate::ecs::storage::join::join;
    use crate::ecs::storage::section::testing::{apply, encode_changes, sim, uid, values};
    use crate::ecs::world::World;
    use crate::net::wire::{DecodeErrorKind, Encode, de, en};

    /// The `Stats` of the wire format's patch example, written as protoc
    /// would generate it:
    ///
    /// ```text
    /// @replicate(patch)
    /// component Stats {
    ///     alive: bool,
    ///     hp: u16,
    ///     level: option<u8>,
    ///     title: option<string[max_length=16]>,
    ///     name: string[max_length=12],
    /// }
    /// ```
    #[derive(Component, Debug, Clone, PartialEq, Default)]
    #[component(storage = PatchTable<Self>)]
    struct Stats {
        alive: bool,
        hp: u16,
        level: Option<u8>,
        title: Option<String>,
        name: String,
    }

    impl Replicated for Stats {
        const WIRE_ID: u16 = 5;
        const VISIBILITY: Visibility = Visibility::Public;
    }

    impl Valued for Stats {
        const SIZE: Option<usize> = None;
    }

    // bitmap: alive, level, title | hp | boundary | [level] | title | name
    impl Encode for Stats {
        const MAX_ENCODED_SIZE: Option<usize> = Some(33);

        fn encoded_size(&self) -> usize {
            let title = self.title.as_ref().map_or(0, String::len);
            4 + self.level.is_some() as usize + title + self.name.len()
        }

        fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
            en::encode(self, buf, "Stats")
        }

        fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
            let start = w.pos();
            let bits = self.alive as u8
                | (self.level.is_some() as u8) << 1
                | (self.title.is_some() as u8) << 2;
            w.put(&[bits])?;
            w.put(&self.hp.to_le_bytes())?;
            let boundary = w.zeros(1)?;
            if let Some(level) = self.level {
                w.put(&[level])?;
            }
            if let Some(title) = &self.title {
                w.put(title.as_bytes())?;
            }
            w.end(boundary, start, 1)?;
            w.put(self.name.as_bytes())
        }
    }

    fn text(bytes: &[u8], at: usize, max: usize) -> Result<String, DecodeError> {
        if bytes.len() > max {
            return Err(DecodeError::new(at, "Stats", DecodeErrorKind::Length));
        }
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| DecodeError::new(at, "Stats", DecodeErrorKind::Utf8))
    }

    /// The slots `[at, boundary)` and `[boundary, len)`.
    fn slots(buf: &[u8], at: usize, boundary: usize) -> Result<(&[u8], &[u8]), DecodeError> {
        if boundary < at || boundary > buf.len() {
            return Err(DecodeError::new(at - 1, "Stats", DecodeErrorKind::Boundary));
        }
        Ok((&buf[at..boundary], &buf[boundary..]))
    }

    impl Decode<'_> for Stats {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            de::at_least(buf, 4, 0, "Stats")?;
            let bits = buf[0];
            de::unused(bits, !0b111, 0, "Stats")?;
            let level = match bits & 0b10 {
                0 => None,
                _ => Some(*buf.get(4).ok_or(DecodeError::new(
                    4,
                    "Stats",
                    DecodeErrorKind::Size,
                ))?),
            };
            let at = 4 + level.is_some() as usize;
            let (title, name) = slots(buf, at, buf[3] as usize)?;
            Ok(Stats {
                alive: bits & 1 != 0,
                hp: u16::from_le_bytes([buf[1], buf[2]]),
                level,
                title: match bits & 0b100 {
                    0 => de::empty(title, at, "Stats").map(|()| None)?,
                    _ => Some(text(title, at, 16)?),
                },
                name: text(name, buf[3] as usize, 12)?,
            })
        }
    }

    /// The received patch, owned: `None` for a field it leaves as is.
    struct StatsPatch {
        alive: Option<bool>,
        hp: Option<u16>,
        level: Option<Option<u8>>,
        title: Option<Option<String>>,
        name: Option<String>,
    }

    // bitmap: 5 dirty bits, alive, level Some, title Some | boundary
    //   | [hp] | [level] | title | name
    impl Decode<'_> for StatsPatch {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            de::at_least(buf, 2, 0, "StatsPatch")?;
            let bits = buf[0];
            let sent = |field: u32| bits & 1 << field != 0;
            // a value bit without its dirty bit
            for (field, extra) in [(0, 5), (2, 6), (3, 7)] {
                if !sent(field) && sent(extra) {
                    return Err(DecodeError::new(0, "StatsPatch", DecodeErrorKind::Padding));
                }
            }
            let mut at = 2;
            let mut take = |len: usize| {
                let bytes = buf.get(at..at + len);
                at += len;
                bytes.ok_or(DecodeError::new(
                    at - len,
                    "StatsPatch",
                    DecodeErrorKind::Size,
                ))
            };
            let hp = sent(1)
                .then(|| take(2).map(|b| u16::from_le_bytes([b[0], b[1]])))
                .transpose()?;
            let level = match (sent(2), sent(6)) {
                (false, _) => None,
                (true, false) => Some(None),
                (true, true) => Some(Some(take(1)?[0])),
            };
            let (title, name) = slots(buf, at, buf[1] as usize)?;
            if !(sent(3) && sent(7)) {
                de::empty(title, at, "StatsPatch")?;
            }
            if !sent(4) {
                de::empty(name, buf[1] as usize, "StatsPatch")?;
            }
            Ok(StatsPatch {
                alive: sent(0).then_some(sent(5)),
                hp,
                level,
                title: match (sent(3), sent(7)) {
                    (false, _) => None,
                    (true, false) => Some(None),
                    (true, true) => Some(Some(text(title, at, 16)?)),
                },
                name: sent(4)
                    .then(|| text(name, buf[1] as usize, 12))
                    .transpose()?,
            })
        }
    }

    impl Patch for Stats {
        const FIELDS: usize = 5;
        const MAX_PATCH_SIZE: Option<usize> = Some(33);

        type Mut<'a> = StatsMut<'a>;
        type Patch<'a> = StatsPatch;

        fn new_mut<'a>(value: &'a mut Self, mask: FieldMask<'a, Self>) -> StatsMut<'a> {
            StatsMut { value, mask }
        }

        fn patch_size(&self, mask: &[u8]) -> usize {
            let sent = |field: u32| mask[0] & 1 << field != 0;
            let mut size = 2;
            if sent(1) {
                size += 2;
            }
            if sent(2) && self.level.is_some() {
                size += 1;
            }
            if sent(3) {
                size += self.title.as_ref().map_or(0, String::len);
            }
            if sent(4) {
                size += self.name.len();
            }
            size
        }

        fn encode_patch(&self, mask: &[u8], buf: &mut [u8]) -> Result<usize, EncodeError> {
            let sent = |field: u32| mask[0] & 1 << field != 0;
            let mut bits = mask[0];
            if sent(0) && self.alive {
                bits |= 1 << 5;
            }
            if sent(2) && self.level.is_some() {
                bits |= 1 << 6;
            }
            if sent(3) && self.title.is_some() {
                bits |= 1 << 7;
            }
            let mut w = en::Writer::new(buf, "StatsPatch");
            w.put(&[bits])?;
            let boundary = w.zeros(1)?;
            if sent(1) {
                w.put(&self.hp.to_le_bytes())?;
            }
            if let (true, Some(level)) = (sent(2), self.level) {
                w.put(&[level])?;
            }
            if let (true, Some(title)) = (sent(3), &self.title) {
                w.put(title.as_bytes())?;
            }
            w.end(boundary, 0, 1)?;
            if sent(4) {
                w.put(self.name.as_bytes())?;
            }
            Ok(w.pos())
        }

        fn apply(&mut self, patch: StatsPatch) {
            if let Some(alive) = patch.alive {
                self.alive = alive;
            }
            if let Some(hp) = patch.hp {
                self.hp = hp;
            }
            if let Some(level) = patch.level {
                self.level = level;
            }
            if let Some(title) = patch.title {
                self.title = title;
            }
            if let Some(name) = patch.name {
                self.name = name;
            }
        }
    }

    struct StatsMut<'a> {
        value: &'a mut Stats,
        mask: FieldMask<'a, Stats>,
    }

    impl StatsMut<'_> {
        fn alive_mut(&mut self) -> &mut bool {
            self.mask.mark(0);
            &mut self.value.alive
        }

        fn hp(&self) -> u16 {
            self.value.hp
        }

        fn hp_mut(&mut self) -> &mut u16 {
            self.mask.mark(1);
            &mut self.value.hp
        }

        fn level_mut(&mut self) -> &mut Option<u8> {
            self.mask.mark(2);
            &mut self.value.level
        }

        fn title_mut(&mut self) -> &mut Option<String> {
            self.mask.mark(3);
            &mut self.value.title
        }

        fn name_mut(&mut self) -> &mut String {
            self.mask.mark(4);
            &mut self.value.name
        }

        fn as_mut_all(&mut self) -> &mut Stats {
            self.mask.mark_all();
            self.value
        }
    }

    impl Deref for StatsMut<'_> {
        type Target = Stats;

        fn deref(&self) -> &Stats {
            self.value
        }
    }

    /// Ten one-byte fields, for a mask of two bytes.
    #[derive(Component, Debug, Clone, PartialEq, Default)]
    #[component(storage = PatchTable<Self>)]
    struct Wide([u8; 10]);

    impl Replicated for Wide {
        const WIRE_ID: u16 = 6;
        const VISIBILITY: Visibility = Visibility::Owner;
    }

    impl Valued for Wide {
        const SIZE: Option<usize> = Some(10);
    }

    impl Encode for Wide {
        const MAX_ENCODED_SIZE: Option<usize> = Some(10);

        fn encoded_size(&self) -> usize {
            10
        }

        fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
            en::encode(self, buf, "Wide")
        }

        fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
            w.put(&self.0)
        }
    }

    impl Decode<'_> for Wide {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            de::exact(buf, 10, 0, "Wide")?;
            Ok(Wide(buf.try_into().unwrap()))
        }
    }

    /// A patch's fields, `(field, value)`.
    struct WidePatch(Vec<(usize, u8)>);

    // bitmap of 10 dirty bits | the sent fields, packed
    impl Decode<'_> for WidePatch {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            de::at_least(buf, 2, 0, "WidePatch")?;
            de::unused(buf[1], !0b11, 1, "WidePatch")?;
            let sent = (0..10).filter(|&field| buf[field / 8] & 1 << (field % 8) != 0);
            let fields = sent.collect::<Vec<_>>();
            de::exact(buf, 2 + fields.len(), 0, "WidePatch")?;
            Ok(WidePatch(
                fields.into_iter().zip(buf[2..].iter().copied()).collect(),
            ))
        }
    }

    impl Patch for Wide {
        const FIELDS: usize = 10;
        const MAX_PATCH_SIZE: Option<usize> = Some(12);

        type Mut<'a> = WideMut<'a>;
        type Patch<'a> = WidePatch;

        fn new_mut<'a>(value: &'a mut Self, mask: FieldMask<'a, Self>) -> WideMut<'a> {
            WideMut { value, mask }
        }

        fn patch_size(&self, mask: &[u8]) -> usize {
            2 + mask
                .iter()
                .map(|byte| byte.count_ones() as usize)
                .sum::<usize>()
        }

        fn encode_patch(&self, mask: &[u8], buf: &mut [u8]) -> Result<usize, EncodeError> {
            let mut w = en::Writer::new(buf, "WidePatch");
            w.put(mask)?;
            for (field, value) in self.0.iter().enumerate() {
                if mask[field / 8] & 1 << (field % 8) != 0 {
                    w.put(&[*value])?;
                }
            }
            Ok(w.pos())
        }

        fn apply(&mut self, patch: WidePatch) {
            for (field, value) in patch.0 {
                self.0[field] = value;
            }
        }
    }

    struct WideMut<'a> {
        value: &'a mut Wide,
        mask: FieldMask<'a, Wide>,
    }

    impl WideMut<'_> {
        fn field_mut(&mut self, field: usize) -> &mut u8 {
            self.mask.mark(field);
            &mut self.value.0[field]
        }

        fn as_mut_all(&mut self) -> &mut Wide {
            self.mask.mark_all();
            self.value
        }
    }

    fn sample(hp: u16) -> Stats {
        Stats {
            alive: true,
            hp,
            level: Some(1),
            title: Some("the brave".into()),
            name: "ana".into(),
        }
    }

    fn mask<C: Patch<Storage = PatchTable<C>>>(world: &World, entity: Entity) -> Vec<u8> {
        world.view::<C>().mask(entity).unwrap().to_vec()
    }

    #[test]
    fn patch_layout() {
        crate::ecs::init();
        let mut world = sim();
        let entity = world.spawn();
        world.promote(entity);
        let value = Stats {
            alive: false,
            hp: 30,
            level: None,
            title: Some("sir".into()),
            name: "al".into(),
        };
        world.view_mut::<Stats>().insert(entity, value);
        world.end_tick();

        {
            let mut stats = world.view_mut::<Stats>();
            let mut stats = stats.get_mut(entity).unwrap();
            assert_eq!(stats.hp(), 30);
            *stats.alive_mut() = true;
            *stats.level_mut() = Some(7);
            *stats.title_mut() = None;
            *stats.name_mut() = "bob".into();
        }
        // reading marks nothing
        assert_eq!(mask::<Stats>(&world, entity), [0b11101]);

        // the patch of the wire format's example
        let stats = world.view::<Stats>();
        let value = stats.get(entity).unwrap();
        let mut patch = [0; 33];
        let size = value.encode_patch(&[0b11101], &mut patch).unwrap();
        assert_eq!(size, value.patch_size(&[0b11101]));
        assert_eq!(&patch[..size], [0x7D, 0x03, 0x07, 0x62, 0x6F, 0x62]);
        drop(stats);

        // a dirty entry's patch goes in the changes part, behind a 1-byte len
        let section = encode_changes::<Stats>(&world, &[]);
        #[rustfmt::skip]
        assert_eq!(section, [
            0x05, 0x00,
            0x02,
            0x0F, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x06,
            0x7D, 0x03, 0x07, 0x62, 0x6F, 0x62,
        ]);

        // an entering entity's full value goes in the values part
        let section = encode_changes::<Stats>(&world, &[entity]);
        let mut full = vec![0; 33];
        let size = world
            .view::<Stats>()
            .get(entity)
            .unwrap()
            .encode(&mut full)
            .unwrap();
        assert_eq!(full[..size], [0b011, 30, 0, 5, 7, b'b', b'o', b'b']);
        assert_eq!(section[2], 0x01);
        assert_eq!(section[3..7], (8 + 1 + size as u32).to_le_bytes());
        assert_eq!(section[15], size as u8);
        assert_eq!(section[16..], full[..size]);
    }

    #[test]
    fn patch_hooked_insert() {
        crate::ecs::init();
        let mut world = World::new();
        let removed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        {
            let removed = removed.clone();
            world.on_remove::<Stats>(move |_: &World, values: &mut Vec<(Entity, Stats)>| {
                removed.borrow_mut().extend(values.drain(..));
            });
        }
        let (a, b) = (world.spawn(), world.spawn());
        world.view_mut::<Stats>().insert(a, sample(1));
        world.view_mut::<Stats>().insert(b, sample(2));
        world.end_tick();

        // the hook keeps the replaced value, and the entry is still marked
        assert_eq!(world.view_mut::<Stats>().insert(a, sample(3)), None);
        assert_eq!(mask::<Stats>(&world, a), [0b11111]);
        assert!(!world.view::<Stats>().is_added(a));
        assert_eq!(world.view_mut::<Stats>().remove(a), None);
        assert_eq!(mask::<Stats>(&world, b), [0]);
        world.flush();
        assert_eq!(*removed.borrow(), [(a, sample(1)), (a, sample(3))]);
    }

    #[test]
    fn patch_masks() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            world
                .view_mut::<Stats>()
                .insert(entity, sample(index as u16));
            world.view_mut::<Wide>().insert(entity, Wide::default());
        }
        // an added entry marks no field: it is sent whole
        assert!(world.view::<Stats>().is_added(entities[0]));
        assert_eq!(mask::<Stats>(&world, entities[0]), [0]);
        world.end_tick();

        {
            let mut stats = world.view_mut::<Stats>();
            *stats.get_mut(entities[0]).unwrap().hp_mut() += 1;
            stats.get_mut(entities[1]).unwrap().as_mut_all().hp = 9;
            // insert over a value marks every field
            stats.insert(entities[2], sample(5));
            let mut wides = world.view_mut::<Wide>();
            *wides.get_mut(entities[1]).unwrap().field_mut(9) = 1;
            *wides.get_mut(entities[3]).unwrap().field_mut(2) = 1;
            wides.get_mut(entities[0]).unwrap().as_mut_all().0[0] = 1;
        }
        assert_eq!(mask::<Stats>(&world, entities[0]), [0b00010]);
        assert_eq!(mask::<Stats>(&world, entities[1]), [0b11111]);
        assert_eq!(mask::<Stats>(&world, entities[2]), [0b11111]);
        assert_eq!(mask::<Stats>(&world, entities[3]), [0]);
        assert!(!world.view::<Stats>().is_changed(entities[3]));
        assert_eq!(mask::<Wide>(&world, entities[0]), [0xFF, 0b11]);
        assert_eq!(mask::<Wide>(&world, entities[1]), [0, 0b10]);

        // the last entry's mask moves into the hole, with its bits
        world.view_mut::<Wide>().remove(entities[1]);
        let wides = world.view::<Wide>();
        assert_eq!(wides.entities(), [entities[0], entities[3], entities[2]]);
        assert_eq!(wides.mask(entities[3]), Some(&[0b100, 0][..]));
        assert!(wides.is_changed(entities[3]));
        assert_eq!(wides.mask(entities[2]), Some(&[0, 0][..]));
        assert!(!wides.is_changed(entities[2]));
        drop(wides);
        world.despawn(entities[0]);
        assert_eq!(mask::<Wide>(&world, entities[2]), [0, 0]);
        assert_eq!(mask::<Wide>(&world, entities[3]), [0b100, 0]);
        assert_eq!(world.view::<Wide>().removed(), [entities[1]]);

        world.end_tick();
        for &entity in &entities[1..] {
            assert_eq!(mask::<Stats>(&world, entity), [0]);
        }
        assert_eq!(mask::<Wide>(&world, entities[3]), [0, 0]);
        assert_eq!(world.view::<Stats>().changed().count(), 0);
    }

    #[test]
    fn patch_query() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..6).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            world
                .view_mut::<Stats>()
                .insert(entity, sample(index as u16));
            world.view_mut::<Wide>().insert(entity, Wide::default());
        }
        world.entities().set_replica(entities[5], true);
        world.end_tick();

        // wrappers alive at once share the masks; replicas are skipped
        let mut query = Query::<(Mut<Stats>, Mut<Wide>)>::new(&world);
        let mut items = query.iter().collect::<Vec<_>>();
        assert_eq!(items.len(), 5);
        for (_, (stats, wide)) in &mut items {
            if stats.hp() % 2 == 0 {
                *stats.hp_mut() += 10;
                *wide.field_mut(8) = 1;
            }
        }
        drop(items);
        drop(query);

        let mut query = Query::<(Stats, Changed<Stats>)>::new(&world);
        let mut changed = query.iter().map(|(e, s)| (e, s.hp)).collect::<Vec<_>>();
        changed.sort();
        assert_eq!(
            changed,
            [(entities[0], 10), (entities[2], 12), (entities[4], 14)]
        );
        drop(query);
        assert_eq!(mask::<Stats>(&world, entities[2]), [0b10]);
        assert_eq!(mask::<Wide>(&world, entities[4]), [0, 0b1]);
        assert_eq!(mask::<Stats>(&world, entities[5]), [0]);

        {
            let mut stats = world.view_mut::<Stats>();
            assert!(stats.get_mut(entities[5]).is_none());
            let wides = world.view::<Wide>();
            for (_, (mut stats, _)) in join((&mut stats, &wides)) {
                *stats.name_mut() = "x".into();
            }
        }
        assert_eq!(mask::<Stats>(&world, entities[1]), [0b10000]);
        assert_eq!(mask::<Stats>(&world, entities[5]), [0]);
    }

    #[test]
    fn patch_round_trip() {
        crate::ecs::init();
        let mut sim = sim();
        let edge = World::new();
        let entities = (0..5).map(|_| sim.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            sim.promote(entity);
            sim.view_mut::<Stats>().insert(entity, sample(index as u16));
            sim.view_mut::<Wide>()
                .insert(entity, Wide([index as u8; 10]));
        }
        let uids = entities
            .iter()
            .map(|&e| sim.uid(e).unwrap())
            .collect::<Vec<_>>();

        // the first tick sends every added value
        let section = encode_changes::<Stats>(&sim, &[]);
        assert_eq!(section[2], 0x01);
        apply::<Stats>(&edge, &section);
        apply::<Wide>(&edge, &encode_changes::<Wide>(&sim, &[]));
        assert_eq!(values::<Stats>(&edge), values::<Stats>(&sim));
        assert_eq!(values::<Wide>(&edge), values::<Wide>(&sim));
        sim.end_tick();

        // received state marks nothing on the receiver
        let edges = edge.view::<Stats>();
        assert_eq!(edges.changed().count(), 0);
        let first = edge.resolve(uids[0]).unwrap();
        assert_eq!(edges.mask(first), Some(&[0][..]));
        drop(edges);

        {
            let mut stats = sim.view_mut::<Stats>();
            let mut first = stats.get_mut(entities[0]).unwrap();
            *first.level_mut() = None;
            first.title_mut().take();
            *first.alive_mut() = false;
            *stats.get_mut(entities[1]).unwrap().name_mut() = "zoe".into();
            stats.insert(entities[2], sample(77));
            let mut wides = sim.view_mut::<Wide>();
            *wides.get_mut(entities[3]).unwrap().field_mut(9) = 99;
        }
        sim.view_mut::<Stats>().remove(entities[3]);
        sim.view_mut::<Stats>().remove(entities[4]);
        sim.view_mut::<Stats>().insert(entities[4], sample(44));
        sim.despawn(entities[1]);

        // dirty entries go as patches, the re-added one as a value
        let section = encode_changes::<Stats>(&sim, &[]);
        assert_eq!(section[2], 0b111);
        apply::<Stats>(&edge, &section);
        apply::<Wide>(&edge, &encode_changes::<Wide>(&sim, &[]));
        let mut expected = values::<Stats>(&sim);
        // a despawn is the frame's `gone`, not a removal
        expected.push((uids[1], sample(1)));
        expected.sort_by_key(|(uid, _)| *uid);
        assert_eq!(values::<Stats>(&edge), expected);
        let wide = edge.resolve(uids[3]).unwrap();
        assert_eq!(edge.view::<Wide>().get(wide).unwrap().0[9], 99);

        // a snapshot holds every value
        let snapshot = World::new();
        let alive = sim.view::<Stats>().entities().to_vec();
        apply::<Stats>(&snapshot, &encode_changes::<Stats>(&sim, &alive));
        assert_eq!(values::<Stats>(&snapshot), values::<Stats>(&sim));
    }

    #[test]
    fn patch_apply_errors() {
        crate::ecs::init();
        let mut world = World::new();
        let entity = world.spawn();
        world.view_mut::<Stats>().insert(entity, sample(1));
        world.end_tick();
        let mut table = world.tables[Stats::id() as usize].borrow_mut();
        let mut resolve = |uid| (uid == self::uid(1)).then_some(entity);
        let mut apply = |patch: &[u8]| {
            let mut buf = vec![0b010];
            buf.extend_from_slice(&(8 + 1 + patch.len() as u32).to_le_bytes());
            buf.extend_from_slice(&uid(1).to_bits().to_le_bytes());
            buf.push(patch.len() as u8);
            buf.extend_from_slice(patch);
            table.apply_section(&buf, &mut resolve).map(|_| ())
        };

        // a value bit without its dirty bit, then a truncated patch
        let kind = |result: Result<(), DecodeError>| result.unwrap_err().kind;
        assert_eq!(kind(apply(&[0x20, 0x02])), DecodeErrorKind::Padding);
        assert_eq!(kind(apply(&[0x02, 0x02, 0x01])), DecodeErrorKind::Size);
        assert_eq!(apply(&[0x02, 0x04, 0x09, 0x00]), Ok(()));
        drop(apply);
        drop(table);
        assert_eq!(world.view::<Stats>().get(entity).unwrap().hp, 9);
        assert!(!world.view::<Stats>().is_changed(entity));

        // a patch to a value the receiver does not hold is skipped
        let other = world.spawn();
        let mut buf = vec![0b010, 11, 0, 0, 0];
        buf.extend_from_slice(&uid(1).to_bits().to_le_bytes());
        buf.extend_from_slice(&[2, 0x00, 0x02]);
        let mut table = world.tables[Stats::id() as usize].borrow_mut();
        assert_eq!(
            table.apply_section(&buf, &mut |_| Some(other)),
            Ok(buf.len())
        );
        drop(table);
        assert_eq!(world.view::<Stats>().get(other), None);
    }
}
