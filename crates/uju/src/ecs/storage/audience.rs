use std::any::{Any, type_name};
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::ecs::audience::Audience;
use crate::ecs::entity::{Entity, Uid};
use crate::ecs::hash::FoldBuildHasher;
use crate::ecs::storage::{section, sparse_set::SparseSet, tracked::Tracked};
use crate::net::wire::{DecodeError, DecodeErrorKind, EncodeError, EncodeErrorKind};

/// An audience's table without its type, as the `World` and the runtime
/// drive it.
pub trait KeyTable {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// The audience's [`WIRE_ID`](Audience::WIRE_ID).
    fn wire_id(&self) -> u16;

    /// Drops the keys and observer sets of despawned entities, logging
    /// nothing.
    fn despawn_batch(&mut self, entities: &[Entity]);

    /// Clears the tick's changes: dirty and added bits, the clear log, the
    /// keys at the start of the tick, and the logs of changes.
    fn end_tick(&mut self);

    /// Appends the entities whose key was set this tick and that hold one,
    /// in dense order.
    fn changed(&self, out: &mut Vec<Entity>);

    /// The entities whose key was cleared this tick, in order. Some may be
    /// dead, or hold a key again.
    fn cleared(&self) -> &[Entity];

    /// Whether `entity` holds a key.
    fn has_key(&self, entity: Entity) -> bool;

    /// Formats `entity`'s key with `Debug`, for
    /// [`World::inspect`](crate::ecs::world::World::inspect). Writes nothing
    /// when it holds none.
    fn inspect(&self, entity: Entity, f: &mut fmt::Formatter<'_>) -> fmt::Result;

    /// Appends the encoding of `entity`'s key, and returns whether it holds
    /// one.
    fn encode_key(&self, entity: Entity, out: &mut Vec<u8>) -> Result<bool, EncodeError>;

    /// Appends the encoding of the key `entity` held at the start of the
    /// tick, and returns whether it held one.
    fn encode_start_key(&self, entity: Entity, out: &mut Vec<u8>) -> Result<bool, EncodeError>;

    /// Appends the audience's entry of a frame's `keys` list,
    /// `id:u16 | set | cleared`: the keys of the `set` entities, and the
    /// `cleared` uids.
    ///
    /// Entities that hold no key are skipped, and an entity must not be in
    /// both lists. Returns whether it wrote an entry, which it does not when
    /// both parts are empty.
    fn encode_keys(
        &self,
        set: &[(Uid, Entity)],
        cleared: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError>;

    /// Applies the entry at the start of `buf`, which begins after the
    /// audience id, and returns its length. `resolve` maps each uid to the
    /// entity it lands on, or to `None` to skip it.
    ///
    /// Received keys are not a change of this world: they mark no bit and
    /// log nothing.
    fn apply_keys(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError>;
}

/// An observer's stored set, when it is not the default.
enum Observer<K> {
    Keys(HashSet<K>),
    All,
}

impl<K: Eq + std::hash::Hash> Observer<K> {
    fn same(this: Option<&Self>, other: Option<&Self>) -> bool {
        match (this, other) {
            (None, None) | (Some(Observer::All), Some(Observer::All)) => true,
            (Some(Observer::Keys(this)), Some(Observer::Keys(other))) => this == other,
            _ => false,
        }
    }
}

/// The keys an observer sees for an audience.
#[derive(Debug, Clone, Copy)]
pub enum Observed<'a, K> {
    /// The default: the observer's own key, if it holds one.
    Own(Option<K>),
    /// A set stored by `set_observer_keys`.
    Keys(&'a HashSet<K>),
    /// Every key, stored by `observe_all`.
    All,
}

impl<K: Eq + std::hash::Hash> Observed<'_, K> {
    /// Whether the set holds `key`.
    pub fn contains(&self, key: &K) -> bool {
        match self {
            Observed::Own(own) => own.as_ref() == Some(key),
            Observed::Keys(keys) => keys.contains(key),
            Observed::All => true,
        }
    }
}

/// An audience's table: each entity's key, tracked like a component's
/// value, and the observer sets that are not the default.
///
/// Only the `World`'s audience methods write it. Keys received from their
/// owner are applied by [`KeyTable::apply_keys`], untracked.
pub struct AudienceTable<A: Audience> {
    keys: Tracked<A::Key>,
    /// The key at the start of the tick of each entity whose key changed.
    starts: HashMap<Entity, Option<A::Key>, FoldBuildHasher>,
    /// Every change of the tick, in order, with the key it replaced.
    changes: Vec<(Entity, Option<A::Key>)>,
    observers: SparseSet<Observer<A::Key>>,
    /// The observers whose stored set changed this tick, possibly repeated.
    observers_changed: Vec<Entity>,
}

impl<A: Audience> AudienceTable<A> {
    pub fn new() -> Self {
        Self {
            keys: Tracked::new(),
            starts: HashMap::default(),
            changes: Vec::new(),
            observers: SparseSet::new(),
            observers_changed: Vec::new(),
        }
    }

    /// The number of entities holding a key.
    #[inline]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// `entity`'s key, if it holds one.
    #[inline]
    pub fn key(&self, entity: Entity) -> Option<A::Key> {
        self.keys.get(entity).copied()
    }

    /// The entities holding a key, in dense order.
    #[inline]
    pub fn entities(&self) -> &[Entity] {
        self.keys.entities()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, A::Key)> + '_ {
        self.keys.iter().map(|(entity, &key)| (entity, key))
    }

    /// The keys `observer` sees.
    pub fn observed(&self, observer: Entity) -> Observed<'_, A::Key> {
        match self.observers.get(observer) {
            Some(Observer::Keys(keys)) => Observed::Keys(keys),
            Some(Observer::All) => Observed::All,
            None => Observed::Own(self.key(observer)),
        }
    }

    /// Whether `observer` sees the entities holding `key`.
    pub fn observes(&self, observer: Entity, key: &A::Key) -> bool {
        self.observed(observer).contains(key)
    }

    /// Whether `entity`'s key was set this tick, and it holds one.
    #[inline]
    pub fn is_changed(&self, entity: Entity) -> bool {
        self.keys.is_changed(entity)
    }

    /// Sets or clears `entity`'s key, returning the previous one. Does
    /// nothing when the key is unchanged.
    pub(crate) fn set(&mut self, entity: Entity, key: Option<A::Key>) -> Option<A::Key> {
        let previous = self.key(entity);
        if previous == key {
            return previous;
        }
        self.starts.entry(entity).or_insert(previous);
        self.changes.push((entity, previous));
        match key {
            Some(key) => {
                self.keys.insert(entity, key);
            }
            None => {
                self.keys.remove(entity);
            }
        }
        previous
    }

    /// Stores `observer`'s set of keys.
    pub(crate) fn set_observer_keys(&mut self, observer: Entity, keys: HashSet<A::Key>) {
        self.set_observer(observer, Some(Observer::Keys(keys)));
    }

    /// Makes `observer`'s set every key.
    pub(crate) fn observe_all(&mut self, observer: Entity) {
        self.set_observer(observer, Some(Observer::All));
    }

    /// Returns `observer` to the default set, its own key.
    pub(crate) fn observe_own_key(&mut self, observer: Entity) {
        self.set_observer(observer, None);
    }

    fn set_observer(&mut self, observer: Entity, set: Option<Observer<A::Key>>) {
        if Observer::same(self.observers.get(observer), set.as_ref()) {
            return;
        }
        match set {
            Some(set) => {
                self.observers.insert(observer, set);
            }
            None => {
                self.observers.remove(observer);
            }
        }
        self.observers_changed.push(observer);
    }

    /// The key `entity` held at the start of the tick.
    pub(crate) fn start_key(&self, entity: Entity) -> Option<A::Key> {
        match self.starts.get(&entity) {
            Some(&start) => start,
            None => self.key(entity),
        }
    }

    /// Every change of a key this tick, in order, each with the key it
    /// replaced. An entity's first entry holds its key at the start of the
    /// tick. Some entities may be dead.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the `on_audience_change` hook")
    )]
    pub(crate) fn changes(&self) -> &[(Entity, Option<A::Key>)] {
        &self.changes
    }

    /// The observers whose stored set changed this tick, possibly repeated.
    /// Some may be dead. An observer with the default set also changes its
    /// set when its own key changes, which this does not list.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the owner channel"))]
    pub(crate) fn observers_changed(&self) -> &[Entity] {
        &self.observers_changed
    }

    fn put_key(out: &mut Vec<u8>, key: &A::Key) -> Result<(), EncodeError> {
        let at = out.len();
        out.resize(at + A::KEY_SIZE, 0);
        if let Err(error) = A::encode_key(key, &mut out[at..]) {
            out.truncate(at);
            return Err(error);
        }
        Ok(())
    }

    fn write_keys(
        &self,
        set: &[(Uid, Entity)],
        cleared: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        out.extend_from_slice(&A::WIRE_ID.to_le_bytes());
        let set_len = self.write_part(out, |out| {
            for &(uid, entity) in set {
                if let Some(key) = self.keys.get(entity) {
                    section::put_uid(out, uid);
                    Self::put_key(out, key)?;
                }
            }
            Ok(())
        })?;
        self.write_part(out, |out| {
            for &uid in cleared {
                section::put_uid(out, uid);
            }
            Ok(())
        })?;
        Ok(set_len != 0 || !cleared.is_empty())
    }

    /// Appends `length:u32 | items`, and returns the items' length.
    fn write_part(
        &self,
        out: &mut Vec<u8>,
        items: impl FnOnce(&mut Vec<u8>) -> Result<(), EncodeError>,
    ) -> Result<usize, EncodeError> {
        let part = out.len();
        out.extend_from_slice(&[0; 4]);
        items(out)?;
        let length = out.len() - part - 4;
        let bytes = u32::try_from(length)
            .map_err(|_| EncodeError::new(type_name::<A>(), EncodeErrorKind::TooLarge))?;
        out[part..part + 4].copy_from_slice(&bytes.to_le_bytes());
        Ok(length)
    }

    fn apply(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        let (items, at, next) = read_part::<A>(buf, 0)?;
        section::read_sized::<A>(items, at, Some(A::KEY_SIZE), 0, |uid, bytes| {
            let key = A::decode_key(bytes)?;
            if let Some(entity) = resolve(uid) {
                self.keys.insert_untracked(entity, key);
            }
            Ok(())
        })?;
        let (items, at, next) = read_part::<A>(buf, next)?;
        section::read_uids::<A>(items, at, |uid| {
            if let Some(entity) = resolve(uid) {
                self.keys.remove_untracked(entity);
            }
        })?;
        Ok(next)
    }
}

/// Reads `length:u32 | items` at `at` of `buf`. Returns the items, where
/// they start, and where the part ends.
fn read_part<A>(buf: &[u8], at: usize) -> Result<(&[u8], usize, usize), DecodeError> {
    let size = |offset| DecodeError::new(offset, type_name::<A>(), DecodeErrorKind::Size);
    let length = buf.get(at..at + 4).ok_or_else(|| size(at))?;
    let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
    let start = at + 4;
    let items = buf.get(start..start + length).ok_or_else(|| size(start))?;
    Ok((items, start, start + length))
}

impl<A: Audience> Default for AudienceTable<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Audience> KeyTable for AudienceTable<A> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn wire_id(&self) -> u16 {
        A::WIRE_ID
    }

    fn despawn_batch(&mut self, entities: &[Entity]) {
        for &entity in entities {
            self.keys.remove_untracked(entity);
            self.observers.remove(entity);
        }
    }

    fn end_tick(&mut self) {
        self.keys.end_tick();
        self.starts.clear();
        self.changes.clear();
        self.observers_changed.clear();
    }

    fn changed(&self, out: &mut Vec<Entity>) {
        out.extend(self.keys.changed());
    }

    fn cleared(&self) -> &[Entity] {
        self.keys.removed()
    }

    fn has_key(&self, entity: Entity) -> bool {
        self.keys.contains(entity)
    }

    fn inspect(&self, entity: Entity, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.keys.get(entity) {
            Some(key) => fmt::Debug::fmt(key, f),
            None => Ok(()),
        }
    }

    fn encode_key(&self, entity: Entity, out: &mut Vec<u8>) -> Result<bool, EncodeError> {
        let Some(key) = self.key(entity) else {
            return Ok(false);
        };
        Self::put_key(out, &key)?;
        Ok(true)
    }

    fn encode_start_key(&self, entity: Entity, out: &mut Vec<u8>) -> Result<bool, EncodeError> {
        let Some(key) = self.start_key(entity) else {
            return Ok(false);
        };
        Self::put_key(out, &key)?;
        Ok(true)
    }

    fn encode_keys(
        &self,
        set: &[(Uid, Entity)],
        cleared: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        let start = out.len();
        match self.write_keys(set, cleared, out) {
            Ok(true) => Ok(true),
            Ok(false) => {
                out.truncate(start);
                Ok(false)
            }
            Err(error) => {
                out.truncate(start);
                Err(error)
            }
        }
    }

    fn apply_keys(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        self.apply(buf, resolve)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::UnsafeCell;

    use linkme::distributed_slice;

    use super::*;
    use crate::ecs::audience::{AUDIENCES, Id, Registration, new_table};
    use crate::ecs::storage::section::testing::{sim, uid};
    use crate::ecs::world::World;

    /// `audience Faction: u16`
    struct Faction;

    #[distributed_slice(AUDIENCES)]
    static FACTION: Registration = Registration {
        name: concat!(module_path!(), "::Faction"),
        id: UnsafeCell::new(u16::MAX),
        new_table: new_table::<Faction>,
    };

    impl Audience for Faction {
        type Key = u16;
        const WIRE_ID: u16 = 1;
        const KEY_SIZE: usize = 2;

        fn id() -> Id {
            unsafe { *FACTION.id.get() }
        }

        fn encode_key(key: &u16, buf: &mut [u8]) -> Result<usize, EncodeError> {
            buf[..2].copy_from_slice(&key.to_le_bytes());
            Ok(2)
        }

        fn decode_key(buf: &[u8]) -> Result<u16, DecodeError> {
            Ok(u16::from_le_bytes(buf.try_into().unwrap()))
        }
    }

    /// `audience Stealth: u8 [0..9]`
    struct Stealth;

    #[distributed_slice(AUDIENCES)]
    static STEALTH: Registration = Registration {
        name: concat!(module_path!(), "::Stealth"),
        id: UnsafeCell::new(u16::MAX),
        new_table: new_table::<Stealth>,
    };

    impl Audience for Stealth {
        type Key = u8;
        const WIRE_ID: u16 = 2;
        const KEY_SIZE: usize = 1;

        fn id() -> Id {
            unsafe { *STEALTH.id.get() }
        }

        fn encode_key(key: &u8, buf: &mut [u8]) -> Result<usize, EncodeError> {
            if *key > 9 {
                return Err(EncodeError::new("Stealth", EncodeErrorKind::Range));
            }
            buf[0] = *key;
            Ok(1)
        }

        fn decode_key(buf: &[u8]) -> Result<u8, DecodeError> {
            match buf[0] {
                key @ 0..=9 => Ok(key),
                _ => Err(DecodeError::new(0, "Stealth", DecodeErrorKind::Range)),
            }
        }
    }

    fn changed<A: Audience>(world: &World) -> Vec<Entity> {
        let mut changed = Vec::new();
        world.audiences[A::id() as usize]
            .borrow()
            .changed(&mut changed);
        changed.sort();
        changed
    }

    fn cleared<A: Audience>(world: &World) -> Vec<Entity> {
        world.audiences[A::id() as usize]
            .borrow()
            .cleared()
            .to_vec()
    }

    #[test]
    fn audience_keys_inspected() {
        crate::ecs::init();
        let world = World::new();
        let e = world.spawn();
        world.set_audience_key::<Faction>(e, Some(2));
        world.set_audience_key::<Stealth>(e, Some(7));
        assert_eq!(
            world.inspect(e).to_string(),
            format!(
                "{e}: uid none, cell none, owned\n  \
                 audience Faction: 2\n  audience Stealth: 7"
            )
        );

        world.set_audience_key::<Faction>(e, None);
        let stealth = world.audiences[Stealth::id() as usize].borrow_mut();
        assert_eq!(
            world.inspect(e).to_string(),
            format!("{e}: uid none, cell none, owned\n  audience Stealth: borrowed")
        );
        drop(stealth);
    }

    #[test]
    fn audience_keys() {
        crate::ecs::init();
        let mut world = World::new();
        let e = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        assert_ne!(Faction::id(), Stealth::id());

        assert_eq!(world.set_audience_key::<Faction>(e[0], Some(1)), None);
        world.set_audience_key::<Faction>(e[1], Some(1));
        world.set_audience_key::<Faction>(e[2], Some(2));
        world.set_audience_key::<Stealth>(e[0], Some(3));
        assert_eq!(world.audience_key::<Faction>(e[0]), Some(1));
        assert_eq!(world.audience_key::<Stealth>(e[0]), Some(3));
        assert_eq!(world.audience_key::<Stealth>(e[1]), None);
        assert_eq!(changed::<Faction>(&world), [e[0], e[1], e[2]]);
        world.end_tick();
        assert!(changed::<Faction>(&world).is_empty());
        assert!(world.audience::<Faction>().changes().is_empty());

        // an unchanged key is a no-op
        assert_eq!(world.set_audience_key::<Faction>(e[0], Some(1)), Some(1));
        assert_eq!(world.set_audience_key::<Faction>(e[3], None), None);
        assert!(changed::<Faction>(&world).is_empty());
        assert!(world.audience::<Faction>().changes().is_empty());

        world.set_audience_key::<Faction>(e[0], Some(2));
        world.set_audience_key::<Faction>(e[0], Some(3));
        assert_eq!(world.set_audience_key::<Faction>(e[1], None), Some(1));
        world.set_audience_key::<Faction>(e[3], Some(4));
        world.set_audience_key::<Faction>(e[3], None);
        assert_eq!(changed::<Faction>(&world), [e[0]]);
        assert_eq!(cleared::<Faction>(&world), [e[1]]);

        let table = world.audience::<Faction>();
        assert_eq!(
            table.changes(),
            [
                (e[0], Some(1)),
                (e[0], Some(2)),
                (e[1], Some(1)),
                (e[3], None),
                (e[3], Some(4)),
            ]
        );
        assert_eq!(table.start_key(e[0]), Some(1));
        assert_eq!(table.start_key(e[1]), Some(1));
        assert_eq!(table.start_key(e[2]), Some(2));
        assert_eq!(table.start_key(e[3]), None);
        assert_eq!(table.key(e[0]), Some(3));
        drop(table);

        // a despawn drops the key and logs nothing
        world.end_tick();
        assert!(world.despawn(e[2]));
        assert_eq!(world.audience_key::<Faction>(e[2]), None);
        assert!(cleared::<Faction>(&world).is_empty());
        assert!(world.audience::<Faction>().changes().is_empty());
        assert_eq!(world.set_audience_key::<Faction>(e[2], Some(1)), None);
        assert_eq!(world.audience::<Faction>().len(), 1);
        assert_eq!(world.audience::<Faction>().entities(), [e[0]]);
    }

    #[test]
    fn audience_observers() {
        crate::ecs::init();
        let mut world = World::new();
        let player = world.spawn();
        let gm = world.spawn();
        world.set_audience_key::<Stealth>(player, Some(1));

        // the default set is the observer's own key, and stores nothing
        let table = world.audience::<Stealth>();
        assert!(matches!(table.observed(player), Observed::Own(Some(1))));
        assert!(matches!(table.observed(gm), Observed::Own(None)));
        assert!(table.observes(player, &1));
        assert!(!table.observes(player, &2));
        assert!(!table.observes(gm, &1));
        drop(table);
        world.set_audience_key::<Stealth>(player, Some(2));
        assert!(world.audience::<Stealth>().observes(player, &2));

        world.set_observer_keys::<Stealth>(player, [1, 2, 3]);
        world.observe_all::<Stealth>(gm);
        let table = world.audience::<Stealth>();
        assert!(matches!(table.observed(player), Observed::Keys(keys) if keys.len() == 3));
        assert!(table.observes(player, &3));
        assert!(!table.observes(player, &4));
        assert!(table.observes(gm, &9));
        assert_eq!(table.observers_changed(), [player, gm]);
        drop(table);
        world.end_tick();

        // storing the same set again changes nothing
        world.set_observer_keys::<Stealth>(player, [3, 2, 1]);
        world.observe_all::<Stealth>(gm);
        assert!(world.audience::<Stealth>().observers_changed().is_empty());

        world.observe_own_key::<Stealth>(player);
        world.set_observer_keys::<Stealth>(gm, []);
        let table = world.audience::<Stealth>();
        assert!(matches!(table.observed(player), Observed::Own(Some(2))));
        assert!(!table.observes(gm, &2));
        assert_eq!(table.observers_changed(), [player, gm]);
        drop(table);

        // a despawn drops the set
        world.despawn(gm);
        let recycled = world.spawn();
        assert_eq!(recycled.index(), gm.index());
        assert!(matches!(
            world.audience::<Stealth>().observed(recycled),
            Observed::Own(None)
        ));
        world.observe_all::<Stealth>(gm);
        assert!(!world.audience::<Stealth>().observes(gm, &1));
    }

    /// Encodes `A`'s entry of the `keys` list as the encoder does, with the
    /// entities in `entered` sent whole.
    fn encode_keys<A: Audience>(sim: &World, entered: &[Entity]) -> Vec<u8> {
        let table = sim.audiences[A::id() as usize].borrow();
        let mut set = entered.to_vec();
        table.changed(&mut set);
        set.sort();
        set.dedup();
        let set = set
            .into_iter()
            .map(|e| (sim.uid(e).unwrap(), e))
            .collect::<Vec<_>>();
        let cleared = table
            .cleared()
            .iter()
            .filter(|&&e| sim.is_alive(e) && sim.audience_key::<A>(e).is_none())
            .filter_map(|&e| sim.uid(e))
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        table.encode_keys(&set, &cleared, &mut out).unwrap();
        out
    }

    fn apply_keys<A: Audience>(edge: &World, entry: &[u8]) -> Result<usize, DecodeError> {
        let mut table = edge.audiences[A::id() as usize].borrow_mut();
        assert_eq!(u16::from_le_bytes([entry[0], entry[1]]), table.wire_id());
        let mut resolve = |uid| {
            let entity = edge.resolve(uid).or_else(|| edge.adopt(uid))?;
            edge.entities().set_replica(entity, true);
            Some(entity)
        };
        table.apply_keys(&entry[2..], &mut resolve)
    }

    #[test]
    fn audience_keys_round_trip() {
        crate::ecs::init();
        let mut sim = sim();
        let edge = World::new();
        let e = (0..3).map(|_| sim.spawn()).collect::<Vec<_>>();
        for &entity in &e {
            sim.promote(entity);
        }
        sim.set_audience_key::<Stealth>(e[0], Some(1));
        sim.set_audience_key::<Stealth>(e[1], Some(2));

        let entry = encode_keys::<Stealth>(&sim, &[]);
        #[rustfmt::skip]
        assert_eq!(entry, [
            2, 0,                       // audience 2
            18, 0, 0, 0,                // set: 18 bytes
            0, 0, 0, 0, 1, 0, 0, 0, 1,  //   1:0, key 1
            1, 0, 0, 0, 1, 0, 0, 0, 2,  //   1:1, key 2
            0, 0, 0, 0,                 // cleared: none
        ]);
        assert_eq!(apply_keys::<Stealth>(&edge, &entry), Ok(entry.len() - 2));
        let replica = edge.resolve(uid(0)).unwrap();
        assert_eq!(edge.audience_key::<Stealth>(replica), Some(1));
        assert_eq!(
            edge.audience_key::<Stealth>(edge.resolve(uid(1)).unwrap()),
            Some(2)
        );
        // received keys are untracked
        assert!(changed::<Stealth>(&edge).is_empty());
        assert!(edge.audience::<Stealth>().changes().is_empty());
        sim.end_tick();

        // nothing changed: no entry
        assert!(encode_keys::<Stealth>(&sim, &[]).is_empty());

        sim.set_audience_key::<Stealth>(e[0], Some(3));
        sim.set_audience_key::<Stealth>(e[1], None);
        let entry = encode_keys::<Stealth>(&sim, &[e[2]]);
        #[rustfmt::skip]
        assert_eq!(entry, [
            2, 0,
            9, 0, 0, 0,
            0, 0, 0, 0, 1, 0, 0, 0, 3,
            8, 0, 0, 0,
            1, 0, 0, 0, 1, 0, 0, 0,
        ]);
        apply_keys::<Stealth>(&edge, &entry).unwrap();
        assert_eq!(edge.audience_key::<Stealth>(replica), Some(3));
        assert_eq!(
            edge.audience_key::<Stealth>(edge.resolve(uid(1)).unwrap()),
            None
        );
        assert!(cleared::<Stealth>(&edge).is_empty());

        // sub-frame ids: the key now and at the start of the tick
        let table = sim.audiences[Stealth::id() as usize].borrow();
        let mut out = Vec::new();
        assert!(table.encode_key(e[0], &mut out).unwrap());
        assert!(table.encode_start_key(e[0], &mut out).unwrap());
        assert!(!table.encode_key(e[1], &mut out).unwrap());
        assert!(table.encode_start_key(e[1], &mut out).unwrap());
        assert!(!table.encode_start_key(e[2], &mut out).unwrap());
        assert_eq!(out, [3, 1, 2]);
    }

    #[test]
    fn audience_keys_errors() {
        crate::ecs::init();
        let sim = sim();
        let edge = World::new();
        let entity = sim.spawn();
        sim.promote(entity);

        // a key that does not encode leaves nothing behind
        sim.set_audience_key::<Stealth>(entity, Some(10));
        let table = sim.audiences[Stealth::id() as usize].borrow();
        let mut out = vec![7];
        let set = [(uid(0), entity)];
        assert!(table.encode_keys(&set, &[], &mut out).is_err());
        assert!(table.encode_key(entity, &mut out).is_err());
        assert_eq!(out, [7]);
        drop(table);

        let kind = |entry: &[u8]| apply_keys::<Stealth>(&edge, entry).unwrap_err().kind;
        // a truncated part
        assert_eq!(kind(&[2, 0, 9, 0, 0, 0, 1, 0]), DecodeErrorKind::Size);
        // an item cut short
        let short = [2, 0, 8, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(kind(&short), DecodeErrorKind::Size);
        // a key out of range
        let range = [2, 0, 9, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 10, 0, 0, 0, 0];
        assert_eq!(kind(&range), DecodeErrorKind::Range);
        // a zero incarnation
        let zero = [2, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(kind(&zero), DecodeErrorKind::Range);
        // a cleared part that is not whole uids
        assert_eq!(
            kind(&[2, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]),
            DecodeErrorKind::Length
        );
        assert!(edge.entities().is_empty());
    }
}
