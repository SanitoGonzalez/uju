use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::fmt;

use crate::ecs::{
    audience::{self, Audience},
    bundle::Bundle,
    cell::Cell,
    commands::Commands,
    component::{self, Component, Ops, Required},
    entity::{Despawned, Entity, IncarnationSwitched, Registry, Uid, UidAllocator},
    event::{self, Event, Queue},
    hook::{ComponentHooks, Hooks, Pending},
    message::{self, EntityMessage, InboxDrain, Mailbox},
    name::Name,
    storage::{
        Storage, Tracking,
        audience::{AudienceTable, KeyTable},
        ops::OpsTable,
        table::Table,
        view::{View, ViewMut},
    },
    unique::{self, Unique},
};

/// A shard's entities, component tables, audience tables, uniques, event
/// queues, inboxes and component hooks.
///
/// A world is built on its shard's thread and never leaves it.
pub struct World {
    pub(crate) entities: Registry,
    pub(crate) tables: Vec<RefCell<Box<dyn Table>>>,
    pub(crate) audiences: Vec<RefCell<Box<dyn KeyTable>>>,
    pub(crate) uniques: Vec<RefCell<Option<Box<dyn Any>>>>,
    events: Vec<RefCell<Box<dyn Queue>>>,
    mailboxes: Vec<RefCell<Box<dyn Any>>>,
    commands: RefCell<Commands>,
    despawns: RefCell<Vec<Despawned>>,
    /// The hooked components, by id.
    hooks: Vec<(component::Id, RefCell<Box<dyn Hooks>>)>,
    hooks_pending: Pending,
    flushing: std::cell::Cell<bool>,
}

impl World {
    /// A world with no [`UidAllocator`]: it [`adopt`](Self::adopt)s uids but cannot
    /// [`promote`](Self::promote) until one is installed.
    pub fn new() -> Self {
        Self {
            entities: Registry::new(),
            tables: component::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_table)()))
                .collect(),
            audiences: audience::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_table)()))
                .collect(),
            uniques: (0..unique::count()).map(|_| RefCell::new(None)).collect(),
            events: event::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_queue)()))
                .collect(),
            mailboxes: message::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_mailbox)()))
                .collect(),
            commands: RefCell::new(Commands::default()),
            despawns: RefCell::default(),
            hooks: Vec::new(),
            hooks_pending: Pending::default(),
            flushing: std::cell::Cell::new(false),
        }
    }

    pub fn entities(&self) -> &Registry {
        &self.entities
    }

    pub fn spawn(&self) -> Entity {
        self.entities.spawn()
    }

    /// Spawns an entity holding `bundle`, at once, then adds the components
    /// they [require](Component::required). Borrows each table once.
    ///
    /// Panics if one of the tables is borrowed at the time, such as by a
    /// query. Inside a query's loop, `spawn` then
    /// [`commands().insert`](Commands::insert) defers the inserts.
    pub fn spawn_with<B: Bundle>(&self, bundle: B) -> Entity {
        let entity = self.spawn();
        bundle.insert(self, entity);
        Required::add_to::<B>(self, entity);
        entity
    }

    /// Frees `entity` and drops its components, audience keys and observer
    /// sets.
    ///
    /// Panics if any view or query is borrowed at the time.
    pub fn despawn(&self, entity: Entity) -> bool {
        self.despawn_batch(std::slice::from_ref(&entity)) == 1
    }

    /// Despawns every live entity of `entities`, borrowing each table once,
    /// and returns how many it despawned. Dead entities and repeats are
    /// skipped.
    ///
    /// Panics if any view or query is borrowed at the time.
    pub fn despawn_batch(&self, entities: &[Entity]) -> usize {
        let despawned = self
            .entities
            .despawn_batch(entities, &mut self.despawns.borrow_mut());
        if despawned == 0 {
            return 0;
        }

        // a dead entity's probe misses, so the whole slice goes to each table
        for table in &self.tables {
            let mut table = table.borrow_mut();
            if !table.is_empty() {
                table.despawn_batch(entities);
            }
        }
        for table in &self.audiences {
            table.borrow_mut().despawn_batch(entities);
        }
        despawned
    }

    /// The queue of deferred changes, applied by [`flush`](Self::flush). It
    /// is a borrow of its own, so it is available inside a query's loop.
    ///
    /// Panics if the queue is borrowed at the time, such as by a
    /// `Commands` held across this call.
    pub fn commands(&self) -> RefMut<'_, Commands> {
        self.commands.borrow_mut()
    }

    /// Applies the queued [`commands`](Self::commands), in order, then runs
    /// the [hooks](Self::on_insert) of what changed since the last flush,
    /// and repeats until nothing is queued and no hook has work left.
    /// Commands queued while applying, such as by a closure, apply in the
    /// same flush.
    ///
    /// Called inside a hook or a command, it only applies the queue: the
    /// flush already running runs the hooks.
    ///
    /// Panics if a command conflicts with a live borrow, such as a view of a
    /// table it changes, if a hooked table with work for its hooks is
    /// borrowed, or if the queue is borrowed at the time.
    pub fn flush(&self) {
        if self.flushing.get() {
            self.apply_commands();
            return;
        }
        if self.commands.borrow().is_empty() && !self.hooks_pending.get() {
            return;
        }

        self.flushing.set(true);
        let _flushing = Flushing(&self.flushing);
        loop {
            self.apply_commands();
            if !self.hooks_pending.take() {
                break;
            }
            self.run_hooks();
        }
    }

    fn apply_commands(&self) {
        // keep the queue unborrowed while applying, so commands can queue more
        let mut commands = Commands::default();
        loop {
            std::mem::swap(&mut *self.commands.borrow_mut(), &mut commands);
            if commands.is_empty() {
                break;
            }
            commands.apply(self);
        }
    }

    /// Runs one round of hooks: takes every hooked table's log, then runs
    /// the `on_remove` hooks, then the `on_insert` ones, each in component
    /// order. What they change is logged for the next round.
    fn run_hooks(&self) {
        for (id, hooks) in &self.hooks {
            hooks
                .borrow_mut()
                .take(&mut **self.tables[*id as usize].borrow_mut());
        }
        for (_, hooks) in &self.hooks {
            hooks.borrow_mut().run_on_remove(self);
        }
        for (_, hooks) in &self.hooks {
            hooks.borrow_mut().run_on_insert(self);
        }
    }

    /// Runs `hook` at every flush with the entities that gained a `C` since
    /// the last one, whatever the cause: a view, a command, a bundle, or
    /// state received for a replica. An entity that lost its `C` again
    /// before the flush is skipped, and each comes once, in the order they
    /// gained it. An insert over a value counts, as a removal then an
    /// insert.
    ///
    /// Hooks of one component run in order of registration. Only changes
    /// made after the registration are seen.
    pub fn on_insert<C: Component>(
        &mut self,
        hook: impl FnMut(&World, &[Entity]) + 'static,
    ) -> &mut Self {
        self.component_hooks::<C>().on_insert.push(Box::new(hook));
        self.enable_hook_log::<C>();
        self
    }

    /// Runs `hook` at every flush with the values of `C` removed since the
    /// last one, with their entities, in removal order, whatever the cause:
    /// a view, a command, a despawn, or state received for a replica. An
    /// insert over a value removes it. A despawned entity's values come
    /// too, with the entity dead by then.
    ///
    /// `C`'s table keeps the values until the flush, so its `remove` and
    /// `insert` hand none back. Each hook may take values out of the `Vec`.
    /// The next hook of `C` gets what is left, and the rest is dropped after
    /// the last. Hooks of one component run in order of registration. Only
    /// changes made after the registration are seen.
    pub fn on_remove<C: Component>(
        &mut self,
        hook: impl FnMut(&World, &mut Vec<(Entity, C)>) + 'static,
    ) -> &mut Self {
        self.component_hooks::<C>().on_remove.push(Box::new(hook));
        self.enable_hook_log::<C>();
        self
    }

    fn component_hooks<C: Component>(&mut self) -> &mut ComponentHooks<C> {
        let at = match self.hooks.binary_search_by_key(&C::id(), |(id, _)| *id) {
            Ok(at) => at,
            Err(at) => {
                let hooks: Box<dyn Hooks> = Box::new(ComponentHooks::<C>::new());
                self.hooks.insert(at, (C::id(), RefCell::new(hooks)));
                at
            }
        };
        self.hooks[at]
            .1
            .get_mut()
            .as_any_mut()
            .downcast_mut()
            .unwrap()
    }

    fn enable_hook_log<C: Component>(&mut self) {
        let hooks = self.component_hooks::<C>();
        let (inserts, removes) = (!hooks.on_insert.is_empty(), !hooks.on_remove.is_empty());
        let table = self.tables[C::id() as usize].get_mut();
        let table = table.as_any_mut().downcast_mut::<C::Storage>().unwrap();
        table
            .hook_log()
            .enable(inserts, removes, &self.hooks_pending);
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// See [`Registry::install_uid_allocator`].
    pub fn install_uid_allocator(&self, allocator: UidAllocator) -> Option<UidAllocator> {
        self.entities.install_uid_allocator(allocator)
    }

    /// See [`Registry::uid_allocator_mut`]. The runtime sets the next spare through it.
    ///
    /// Panics if the entity allocator is borrowed at the time.
    pub fn uid_allocator_mut(&self) -> Option<RefMut<'_, UidAllocator>> {
        self.entities.uid_allocator_mut()
    }

    /// See [`Registry::uid`].
    pub fn uid(&self, entity: Entity) -> Option<Uid> {
        self.entities.uid(entity)
    }

    /// The [`Uid`] of `entity`, minting one on first use. `None` if `entity` is dead, or
    /// if it has no uid and no allocator is installed.
    ///
    /// When minting switches the allocator to its spare, emits an
    /// [`IncarnationSwitched`] naming the incarnation it switched from.
    ///
    /// Panics if the installed allocator's current incarnation has run out and it has no
    /// spare (see [`UidAllocator::alloc`]), or if the entity allocator, or the
    /// `IncarnationSwitched` queue on a switch, is borrowed at the time.
    pub fn promote(&self, entity: Entity) -> Option<Uid> {
        let uid = self.entities.promote(entity)?;
        // a switch mints the first serial of the spare
        if uid.serial() == 0 {
            let retired = self
                .uid_allocator_mut()
                .map(|mut allocator| allocator.take_retired())
                .unwrap_or_default();
            for from in retired {
                self.emit(IncarnationSwitched { from });
            }
        }
        Some(uid)
    }

    /// See [`Registry::resolve`].
    pub fn resolve(&self, uid: Uid) -> Option<Entity> {
        self.entities.resolve(uid)
    }

    /// See [`Registry::adopt`].
    pub fn adopt(&self, uid: Uid) -> Option<Entity> {
        self.entities.adopt(uid)
    }

    /// See [`Registry::cell`].
    pub fn cell(&self, entity: Entity) -> Option<Cell> {
        self.entities.cell(entity)
    }

    /// See [`Registry::is_replica`].
    pub fn is_replica(&self, entity: Entity) -> bool {
        self.entities.is_replica(entity)
    }

    /// Formats `entity` for debugging: its uid, cell and replica flag on
    /// the first line, then one line per component it holds, its
    /// [`Name`] first, then one per audience key. Components show through
    /// [`Component::inspect`], audience keys through `Debug`. A dead entity
    /// shows as `dead`.
    ///
    /// ```text
    /// 3v1: uid 7:0, cell 12, owned
    ///   Name("orc")
    ///   Hp(10)
    ///   audience Faction: 2
    /// ```
    ///
    /// The value reads the world when formatted, borrowing one table at a
    /// time. A table borrowed mutably at that point, such as by the query
    /// being iterated, shows as `Hp: borrowed`, whether the entity holds a
    /// value or not.
    pub fn inspect(&self, entity: Entity) -> Inspect<'_> {
        Inspect {
            world: self,
            entity,
        }
    }

    /// The owned entities despawned this tick that had a uid and a cell at
    /// the start of it, in despawn order.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the encoder"))]
    pub(crate) fn despawns(&self) -> Ref<'_, [Despawned]> {
        Ref::map(self.despawns.borrow(), Vec::as_slice)
    }

    /// Ends the tick: rolls each entity's cell into its cell at the start of
    /// the tick, and clears the despawn log, every table's changes, audience
    /// tables included, and every event queue. Inboxes keep what is unread.
    #[cfg_attr(not(test), expect(dead_code, reason = "called by the runtime's tick"))]
    pub(crate) fn end_tick(&mut self) {
        self.entities.end_tick();
        self.despawns.get_mut().clear();
        for queue in &mut self.events {
            queue.get_mut().clear();
        }
        for table in &mut self.tables {
            table.get_mut().end_tick();
        }
        for table in &mut self.audiences {
            table.get_mut().end_tick();
        }
    }

    /// Pushes `event` onto `E`'s queue, at once.
    ///
    /// Panics if `E`'s queue is borrowed at the time, such as by a loop over
    /// [`events`](Self::events) of `E`.
    pub fn emit<E: Event>(&self, event: E) {
        self.event_queue_mut::<E>().push(event);
    }

    /// The events of `E` emitted this tick, in order. Any number of readers
    /// can hold the queue at once.
    ///
    /// Panics if `E`'s queue is borrowed mutably at the time.
    pub fn events<E: Event>(&self) -> Ref<'_, [E]> {
        Ref::map(self.events[E::id() as usize].borrow(), |queue| {
            let queue: &dyn Any = &**queue;
            queue.downcast_ref::<Vec<E>>().unwrap().as_slice()
        })
    }

    fn event_queue_mut<E: Event>(&self) -> RefMut<'_, Vec<E>> {
        RefMut::map(self.events[E::id() as usize].borrow_mut(), |queue| {
            let queue: &mut dyn Any = &mut **queue;
            queue.downcast_mut().unwrap()
        })
    }

    /// Sends `message` to `target`, which this `World` holds owned or as a
    /// replica. A message to an owned entity is pushed onto `M`'s inbox at
    /// once. One to a replica is queued for the runtime, which routes it to
    /// the shard that owns the entity. A message to a dead entity is
    /// dropped.
    ///
    /// Panics if the entity allocator or the replica flags are borrowed
    /// mutably at the time.
    pub fn send<M: EntityMessage + Clone>(&self, target: Entity, message: &M) {
        if !self.is_alive(target) {
            return;
        }
        if !self.entities.replicas().contains(target) {
            self.mailbox_mut::<M>()
                .inbox
                .push_back((target, message.clone()));
        } else if let Some(uid) = self.uid(target) {
            self.mailbox_mut::<M>().outbox.push((uid, message.clone()));
        }
    }

    /// Drains `M`'s inbox: yields each message with its entity, oldest
    /// first, skipping entities that are dead. A message of `M` sent while
    /// the drain is alive stays queued for the next read, and so do the
    /// messages left when the drain is dropped early.
    pub fn inbox<M: EntityMessage>(&self) -> InboxDrain<'_, M> {
        InboxDrain::new(self)
    }

    /// Pushes a message another shard sent onto `M`'s inbox, as the runtime
    /// does when the tick begins.
    #[cfg_attr(not(test), expect(dead_code, reason = "called by the runtime's tick"))]
    pub(crate) fn deliver<M: EntityMessage>(&self, target: Entity, message: M) {
        self.mailbox_mut::<M>().inbox.push_back((target, message));
    }

    /// Takes the messages sent to replicas, with the uid of each target,
    /// oldest first, for the runtime to route.
    #[cfg_attr(not(test), expect(dead_code, reason = "called by the runtime's tick"))]
    pub(crate) fn take_outbox<M: EntityMessage>(&self) -> Vec<(Uid, M)> {
        std::mem::take(&mut self.mailbox_mut::<M>().outbox)
    }

    pub(crate) fn mailbox_mut<M: EntityMessage>(&self) -> RefMut<'_, Mailbox<M>> {
        RefMut::map(self.mailboxes[M::id() as usize].borrow_mut(), |mailbox| {
            mailbox.downcast_mut().unwrap()
        })
    }

    /// Applies `op` to `entity`'s `C` and records it, as
    /// [`OpsMut::apply`](crate::ecs::storage::ops::OpsMut::apply) does.
    /// Returns `false` when `entity` holds no `C` or is a replica.
    ///
    /// Panics if `C`'s table is borrowed at the time, such as by a query.
    pub fn apply_op<C: Ops<Storage = OpsTable<C>>>(&self, entity: Entity, op: C::Op) -> bool {
        let mut table = self.view_mut::<C>();
        let Some(mut value) = table.get_mut(entity) else {
            return false;
        };
        value.apply(op);
        true
    }

    /// Sends `entity`'s full `C` in the next delta, dropping its ops of this
    /// tick, as [`OpsTable::resync`] does. Returns `false` when `entity`
    /// holds no `C` or is a replica.
    ///
    /// Panics if `C`'s table is borrowed at the time, such as by a query.
    pub fn resync<C: Ops<Storage = OpsTable<C>>>(&self, entity: Entity) -> bool {
        let mut table = self.view_mut::<C>();
        !self.is_replica(entity) && table.resync(entity)
    }

    /// `entity`'s key for audience `A`, owned or a replica's.
    ///
    /// Panics if `A`'s table is borrowed mutably at the time.
    pub fn audience_key<A: Audience>(&self, entity: Entity) -> Option<A::Key> {
        self.audience::<A>().key(entity)
    }

    /// Sets `entity`'s key for audience `A`, or clears it with `None`, and
    /// returns the previous one. Does nothing when the key is unchanged or
    /// `entity` is dead.
    ///
    /// Panics if `A`'s table is borrowed at the time.
    pub fn set_audience_key<A: Audience>(
        &self,
        entity: Entity,
        key: Option<A::Key>,
    ) -> Option<A::Key> {
        if !self.is_alive(entity) {
            return None;
        }
        self.audience_mut::<A>().set(entity, key)
    }

    /// Makes `observer` see the entities holding any of `keys` for audience
    /// `A`. Does nothing when `observer` is dead.
    ///
    /// Panics if `A`'s table is borrowed at the time.
    pub fn set_observer_keys<A: Audience>(
        &self,
        observer: Entity,
        keys: impl IntoIterator<Item = A::Key>,
    ) {
        if self.is_alive(observer) {
            let keys = keys.into_iter().collect();
            self.audience_mut::<A>().set_observer_keys(observer, keys);
        }
    }

    /// Makes `observer` see every key of audience `A`. Does nothing when
    /// `observer` is dead.
    ///
    /// Panics if `A`'s table is borrowed at the time.
    pub fn observe_all<A: Audience>(&self, observer: Entity) {
        if self.is_alive(observer) {
            self.audience_mut::<A>().observe_all(observer);
        }
    }

    /// Returns `observer` to the default set of audience `A`: its own key,
    /// which stores nothing. Does nothing when `observer` is dead.
    ///
    /// Panics if `A`'s table is borrowed at the time.
    pub fn observe_own_key<A: Audience>(&self, observer: Entity) {
        if self.is_alive(observer) {
            self.audience_mut::<A>().observe_own_key(observer);
        }
    }

    /// Shared access to audience `A`'s table: its keys and observer sets.
    ///
    /// Panics if `A`'s table is borrowed mutably at the time.
    pub fn audience<A: Audience>(&self) -> Ref<'_, AudienceTable<A>> {
        Ref::map(self.audiences[A::id() as usize].borrow(), |table| {
            table.as_any().downcast_ref().unwrap()
        })
    }

    fn audience_mut<A: Audience>(&self) -> RefMut<'_, AudienceTable<A>> {
        RefMut::map(self.audiences[A::id() as usize].borrow_mut(), |table| {
            table.as_any_mut().downcast_mut().unwrap()
        })
    }

    pub fn insert_unique<U: Unique>(&self, unique: U) -> Option<U> {
        self.uniques[U::id() as usize]
            .borrow_mut()
            .replace(Box::new(unique))
            .map(|previous| *previous.downcast().unwrap())
    }

    pub fn remove_unique<U: Unique>(&self) -> Option<U> {
        self.uniques[U::id() as usize]
            .borrow_mut()
            .take()
            .map(|unique| *unique.downcast().unwrap())
    }

    pub fn get_unique<U: Unique>(&self) -> Option<Ref<'_, U>> {
        Ref::filter_map(self.uniques[U::id() as usize].borrow(), |slot| {
            slot.as_ref().map(|unique| unique.downcast_ref().unwrap())
        })
        .ok()
    }

    pub fn get_unique_mut<U: Unique>(&self) -> Option<RefMut<'_, U>> {
        RefMut::filter_map(self.uniques[U::id() as usize].borrow_mut(), |slot| {
            slot.as_mut().map(|unique| unique.downcast_mut().unwrap())
        })
        .ok()
    }

    pub fn unique<U: Unique>(&self) -> Ref<'_, U> {
        self.get_unique().expect("unique not inserted")
    }

    pub fn unique_mut<U: Unique>(&self) -> RefMut<'_, U> {
        self.get_unique_mut().expect("unique not inserted")
    }

    /// Shared access to `C`'s [`Storage`](Component::Storage).
    pub fn view<C: Component>(&self) -> View<'_, C> {
        View(Ref::map(self.tables[C::id() as usize].borrow(), |table| {
            table.as_any().downcast_ref::<C::Storage>().unwrap()
        }))
    }

    /// Mutable access to `C`'s [`Storage`](Component::Storage).
    pub fn view_mut<C: Component>(&self) -> ViewMut<'_, C> {
        ViewMut {
            storage: RefMut::map(self.tables[C::id() as usize].borrow_mut(), |table| {
                table.as_any_mut().downcast_mut::<C::Storage>().unwrap()
            }),
            registry: &self.entities,
        }
    }

    /// The entities that lost `C` since the tick began, are alive and do not
    /// hold `C` again, in removal order. A despawn is not a removal, so a
    /// despawned entity never shows.
    ///
    /// Holds `C`'s table borrowed shared, and borrows the entity allocator
    /// per entity, so the loop can spawn.
    pub fn removed<C: Component<Storage: Tracking>>(&self) -> impl Iterator<Item = Entity> + '_ {
        let table = self.view::<C>();
        let mut at = 0;
        std::iter::from_fn(move || {
            loop {
                let &entity = Tracking::removed(&*table).get(at)?;
                at += 1;
                if !Table::contains(&*table, entity) && self.is_alive(entity) {
                    return Some(entity);
                }
            }
        })
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

/// [`World::inspect`]'s output, which reads the world when it is formatted.
#[derive(Clone, Copy)]
pub struct Inspect<'w> {
    world: &'w World,
    entity: Entity,
}

impl fmt::Display for Inspect<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { world, entity } = *self;
        write!(f, "{entity}: ")?;
        if !world.is_alive(entity) {
            return f.write_str("dead");
        }
        match world.uid(entity) {
            Some(uid) => write!(f, "uid {uid}")?,
            None => f.write_str("uid none")?,
        }
        match world.cell(entity) {
            Some(cell) => write!(f, ", cell {cell}")?,
            None => f.write_str(", cell none")?,
        }
        f.write_str(if world.is_replica(entity) {
            ", replica"
        } else {
            ", owned"
        })?;

        let names = component::registrations();
        let first = Name::id() as usize;
        let ids = std::iter::once(first).chain((0..world.tables.len()).filter(|&id| id != first));
        for id in ids {
            match world.tables[id].try_borrow() {
                Ok(table) if table.contains(entity) => {
                    f.write_str("\n  ")?;
                    table.inspect(entity, f)?;
                }
                Ok(_) => {}
                Err(_) => write!(f, "\n  {}: borrowed", short_name(names[id].name))?,
            }
        }

        let names = audience::registrations();
        for (table, registration) in world.audiences.iter().zip(names) {
            let name = short_name(registration.name);
            match table.try_borrow() {
                Ok(table) if table.has_key(entity) => {
                    write!(f, "\n  audience {name}: ")?;
                    table.inspect(entity, f)?;
                }
                Ok(_) => {}
                Err(_) => write!(f, "\n  audience {name}: borrowed")?,
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Inspect<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A registered name without its module path.
fn short_name(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// Clears the world's flushing flag when the flush ends, or unwinds.
struct Flushing<'w>(&'w std::cell::Cell<bool>);

impl Drop for Flushing<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::rc::Rc;

    use super::*;
    use crate::ecs::{Component, Unique};

    #[derive(Component, Debug, PartialEq)]
    struct Alpha(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Beta(i32);

    #[derive(Unique, Debug, PartialEq)]
    struct Counter(u32);

    #[derive(Unique, Debug, PartialEq)]
    struct Config(i32);

    #[test]
    fn world_tables() {
        crate::ecs::init();
        let world = World::new();
        let entity = world.spawn();

        assert_ne!(Alpha::id(), Beta::id());

        world.view_mut::<Alpha>().insert(entity, Alpha(1));
        world.view_mut::<Beta>().insert(entity, Beta(2));

        assert_eq!(world.view::<Alpha>().get(entity), Some(&Alpha(1)));
        assert_eq!(world.view::<Beta>().get(entity), Some(&Beta(2)));
    }

    #[test]
    fn world_uniques() {
        crate::ecs::init();
        let world = World::new();

        assert_ne!(Counter::id(), Config::id());
        assert!(world.get_unique::<Counter>().is_none());
        assert!(world.get_unique_mut::<Counter>().is_none());

        assert_eq!(world.insert_unique(Counter(1)), None);
        assert_eq!(world.insert_unique(Config(7)), None);
        assert_eq!(*world.unique::<Counter>(), Counter(1));
        assert_eq!(world.get_unique::<Counter>().as_deref(), Some(&Counter(1)));

        world.unique_mut::<Counter>().0 += 1;
        assert_eq!(*world.unique::<Counter>(), Counter(2));

        world.get_unique_mut::<Counter>().unwrap().0 += 1;
        assert_eq!(*world.unique::<Counter>(), Counter(3));

        assert_eq!(world.insert_unique(Counter(10)), Some(Counter(3)));
        assert_eq!(world.remove_unique::<Counter>(), Some(Counter(10)));
        assert!(world.get_unique::<Counter>().is_none());
        assert_eq!(*world.unique::<Config>(), Config(7));
    }

    #[test]
    fn world_despawn_clears_tables() {
        crate::ecs::init();
        let world = World::new();
        let e0 = world.spawn();
        let e1 = world.spawn();

        world.view_mut::<Alpha>().insert(e0, Alpha(1));
        world.view_mut::<Beta>().insert(e0, Beta(2));
        world.view_mut::<Alpha>().insert(e1, Alpha(3));
        world.install_uid_allocator(UidAllocator::new(NonZeroU32::MIN, NonZeroU32::MAX));
        let u0 = world.promote(e0).unwrap();

        assert!(world.despawn(e0));
        assert!(!world.despawn(e0));
        assert!(!world.is_alive(e0));
        assert_eq!(world.resolve(u0), None);
        assert!(!world.view::<Alpha>().contains(e0));
        assert!(!world.view::<Beta>().contains(e0));
        assert_eq!(world.view::<Alpha>().get(e1), Some(&Alpha(3)));

        // the recycled slot starts empty
        let recycled = world.spawn();
        assert_eq!(recycled.index(), e0.index());
        assert_eq!(world.view::<Alpha>().get(recycled), None);
    }

    #[derive(Component)]
    struct Shared(Rc<i32>);

    #[derive(Unique)]
    struct SharedUnique(Rc<i32>);

    #[test]
    fn world_holds_non_send() {
        crate::ecs::init();
        let world = World::new();
        let entity = world.spawn();
        let value = Rc::new(7);

        world
            .view_mut::<Shared>()
            .insert(entity, Shared(value.clone()));
        world.insert_unique(SharedUnique(value.clone()));
        assert_eq!(*world.view::<Shared>().get(entity).unwrap().0, 7);
        assert_eq!(*world.unique::<SharedUnique>().0, 7);
        assert_eq!(Rc::strong_count(&value), 3);
    }

    #[test]
    fn world_uids() {
        crate::ecs::init();
        let world = World::new();
        let e0 = world.spawn();

        // nothing to mint from until an allocator is installed
        assert_eq!(world.promote(e0), None);
        assert!(world.uid_allocator_mut().is_none());
        let adopted = Uid::new(NonZeroU32::new(9).unwrap(), 42);
        let e1 = world.adopt(adopted).unwrap();
        assert_eq!(world.resolve(adopted), Some(e1));
        assert_eq!(world.uid(e1), Some(adopted));

        let current = NonZeroU32::new(3).unwrap();
        let spare = NonZeroU32::new(4).unwrap();
        let installed = world.install_uid_allocator(UidAllocator::new(current, spare));
        assert!(installed.is_none());
        let u0 = world.promote(e0).unwrap();
        assert_eq!(u0, Uid::new(current, 0));
        assert_eq!(world.uid(e0), Some(u0));
        assert_eq!(world.resolve(u0), Some(e0));

        assert_eq!(world.uid_allocator_mut().unwrap().spare(), Some(spare));
        assert!(world.events::<IncarnationSwitched>().is_empty());
    }

    #[test]
    fn world_despawn_log() {
        crate::ecs::init();
        let mut world = World::new();
        world.install_uid_allocator(UidAllocator::new(NonZeroU32::MIN, NonZeroU32::MAX));
        let (a, b) = (Cell::new(1), Cell::new(2));
        let entities = (0..5).map(|_| world.spawn()).collect::<Vec<_>>();
        let replica = world
            .adopt(Uid::new(NonZeroU32::new(9).unwrap(), 0))
            .unwrap();
        world.entities().set_replica(replica, true);
        world.entities().set_cell(replica, a);
        let uids = entities
            .iter()
            .map(|&entity| {
                world.entities().set_cell(entity, a);
                world.promote(entity).unwrap()
            })
            .collect::<Vec<_>>();
        world.end_tick();

        // the cell at the start of the tick, though it moved since
        world.entities().set_cell(entities[0], b);
        assert!(world.despawn(entities[0]));
        world.despawn_batch(&[entities[1], replica, entities[1]]);
        world.commands().despawn(entities[2]);
        world.flush();

        let logged = |uid, cell| Despawned { uid, cell };
        assert_eq!(
            *world.despawns(),
            [logged(uids[0], a), logged(uids[1], a), logged(uids[2], a)]
        );
        assert_eq!(world.cell(entities[0]), None);
        assert!(!world.is_replica(replica));
        let mut listed = world.entities().cell_index().entities(a).to_vec();
        listed.sort();
        assert_eq!(listed, [entities[3], entities[4]]);

        world.end_tick();
        assert!(world.despawns().is_empty());
        world.despawn(entities[3]);
        assert_eq!(*world.despawns(), [logged(uids[3], a)]);
    }

    #[test]
    fn world_inspect() {
        crate::ecs::init();
        let world = World::new();
        let entity = world.spawn();
        world
            .view_mut::<Shared>()
            .insert(entity, Shared(Rc::new(1)));
        world.view_mut::<Beta>().insert(entity, Beta(-2));
        world.view_mut::<Alpha>().insert(entity, Alpha(1));
        world.view_mut::<Name>().insert(entity, Name::new("orc"));

        // the name first, then by component id, and the type's name alone
        // without `Debug`
        assert_eq!(
            world.inspect(entity).to_string(),
            format!(
                "{entity}: uid none, cell none, owned\n  \
                 Name(\"orc\")\n  Alpha(1)\n  Beta(-2)\n  Shared {{ .. }}"
            )
        );

        // a table borrowed mutably cannot be read
        let replica = world
            .adopt(Uid::new(NonZeroU32::new(9).unwrap(), 4))
            .unwrap();
        world.entities().set_replica(replica, true);
        world.entities().set_cell(replica, Cell::new(12));
        let beta = world.view_mut::<Beta>();
        assert_eq!(
            format!("{:?}", world.inspect(replica)),
            format!("{replica}: uid 9:4, cell 12, replica\n  Beta: borrowed")
        );
        drop(beta);

        world.despawn(entity);
        assert_eq!(world.inspect(entity).to_string(), format!("{entity}: dead"));
    }

    #[test]
    #[should_panic(expected = "unique not inserted")]
    fn world_unique_missing() {
        crate::ecs::init();
        let world = World::new();
        let _ = world.unique::<Counter>();
    }
}
