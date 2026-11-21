use std::any::Any;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::sync::Once;

use linkme::distributed_slice;

use crate::ecs::entity::{Entity, Uid};
use crate::ecs::world::World;

pub type Id = u16;

/// An entity message declared in the schema, as
/// `message(sim -> entity) Hit { ... }`: sent to an entity with
/// [`World::send`] and read by its owner with [`World::inbox`].
pub trait EntityMessage: 'static {
    /// The index of its mailbox in a `World`.
    fn id() -> Id;
}

/// A message type's queues in a `World`.
pub(crate) struct Mailbox<M> {
    /// Messages to owned entities, oldest first.
    pub(crate) inbox: VecDeque<(Entity, M)>,
    /// Messages to replicas, for the runtime to route to their owners.
    pub(crate) outbox: Vec<(Uid, M)>,
}

pub struct Registration {
    pub name: &'static str,
    pub id: UnsafeCell<u16>,
    pub new_mailbox: fn() -> Box<dyn Any>,
}

unsafe impl Sync for Registration {}

/// Builds `M`'s empty mailbox, for [`Registration::new_mailbox`].
pub fn new_mailbox<M: EntityMessage>() -> Box<dyn Any> {
    Box::new(Mailbox::<M> {
        inbox: VecDeque::new(),
        outbox: Vec::new(),
    })
}

#[distributed_slice]
pub static ENTITY_MESSAGES: [Registration];

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        let registrations = registrations();
        assert!(registrations.len() <= u16::MAX as usize);
        for pair in registrations.windows(2) {
            assert_ne!(pair[0].name, pair[1].name, "duplicate entity message name");
        }
        for (index, registration) in registrations.iter().enumerate() {
            unsafe { *registration.id.get() = index as u16 }
        }
    });
}

pub(crate) fn registrations() -> Vec<&'static Registration> {
    let mut registrations: Vec<_> = ENTITY_MESSAGES.iter().collect();
    registrations.sort_unstable_by_key(|registration| registration.name);
    registrations
}

/// The messages of `M`'s inbox, taken out of it by [`World::inbox`]: yields
/// each with its entity, oldest first, skipping entities that are dead.
///
/// The inbox is free while it is read, so a message of `M` sent meanwhile
/// is queued behind and not yielded. Dropped before the end, it puts the
/// messages it did not yield back in front of the queue.
pub struct InboxDrain<'w, M: EntityMessage> {
    world: &'w World,
    messages: VecDeque<(Entity, M)>,
}

impl<'w, M: EntityMessage> InboxDrain<'w, M> {
    pub(crate) fn new(world: &'w World) -> Self {
        let messages = std::mem::take(&mut world.mailbox_mut::<M>().inbox);
        Self { world, messages }
    }
}

impl<M: EntityMessage> Iterator for InboxDrain<'_, M> {
    type Item = (Entity, M);

    fn next(&mut self) -> Option<(Entity, M)> {
        loop {
            let (entity, message) = self.messages.pop_front()?;
            if self.world.is_alive(entity) {
                return Some((entity, message));
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.messages.len()))
    }
}

impl<M: EntityMessage> Drop for InboxDrain<'_, M> {
    fn drop(&mut self) {
        // unread first, then what was sent meanwhile; the taken buffer's
        // capacity goes back to the inbox either way
        let mut mailbox = self.world.mailbox_mut::<M>();
        self.messages.append(&mut mailbox.inbox);
        std::mem::swap(&mut mailbox.inbox, &mut self.messages);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{RefCell, UnsafeCell};
    use std::num::NonZeroU32;
    use std::rc::Rc;

    use linkme::distributed_slice;

    use super::*;
    use crate::ecs::schedule::{Inbox, Schedule, Update};

    macro_rules! messages {
        ($($(#[$doc:meta])* $name:ident($ty:ty) = $registration:ident,)*) => {
            $(
                $(#[$doc])*
                #[derive(Clone, Debug, PartialEq)]
                struct $name($ty);

                #[distributed_slice(ENTITY_MESSAGES)]
                static $registration: Registration = Registration {
                    name: concat!(module_path!(), "::", stringify!($name)),
                    id: UnsafeCell::new(u16::MAX),
                    new_mailbox: new_mailbox::<$name>,
                };

                impl EntityMessage for $name {
                    fn id() -> Id {
                        unsafe { *$registration.id.get() }
                    }
                }
            )*
        };
    }

    messages! {
        /// `message(sim -> entity) Hit { amount: u32 }`
        Hit(u32) = HIT,
        /// `message(sim -> entity) Heal { amount: u32 }`
        Heal(u32) = HEAL,
    }

    fn world() -> World {
        crate::ecs::init();
        World::new()
    }

    fn read<M: EntityMessage>(world: &World) -> Vec<(Entity, M)> {
        world.inbox::<M>().collect()
    }

    #[test]
    fn inbox_drains_in_order() {
        let world = world();
        let (a, b) = (world.spawn(), world.spawn());

        world.send(a, &Hit(1));
        world.send(b, &Hit(2));
        world.send(a, &Heal(3));
        world.send(a, &Hit(4));

        assert_eq!(read(&world), [(a, Hit(1)), (b, Hit(2)), (a, Hit(4))]);
        assert!(read::<Hit>(&world).is_empty());
        assert_eq!(read(&world), [(a, Heal(3))]);
    }

    #[test]
    fn inbox_skips_dead_entities() {
        let world = world();
        let (a, b) = (world.spawn(), world.spawn());

        world.send(a, &Hit(1));
        world.send(b, &Hit(2));
        world.despawn(a);
        // dropped at once
        world.send(a, &Hit(3));

        assert_eq!(read(&world), [(b, Hit(2))]);
        assert!(world.mailbox_mut::<Hit>().inbox.is_empty());
    }

    #[test]
    fn inbox_skips_entities_despawned_while_read() {
        let world = world();
        let (a, b) = (world.spawn(), world.spawn());
        world.send(a, &Hit(1));
        world.send(b, &Hit(2));

        let mut read = Vec::new();
        for (e, hit) in world.inbox::<Hit>() {
            read.push((e, hit));
            world.despawn(b);
        }
        assert_eq!(read, [(a, Hit(1))]);
    }

    #[test]
    fn inbox_send_while_read() {
        let world = world();
        let (a, b) = (world.spawn(), world.spawn());
        world.send(a, &Hit(1));

        // a reflected hit is not read by the loop that sent it
        let mut read = Vec::new();
        for (e, hit) in world.inbox::<Hit>() {
            read.push((e, hit.clone()));
            world.send(b, &Hit(hit.0 + 1));
        }
        assert_eq!(read, [(a, Hit(1))]);
        assert_eq!(self::read(&world), [(b, Hit(2))]);
    }

    #[test]
    fn inbox_dropped_early_keeps_the_rest() {
        let world = world();
        let (a, b, c) = (world.spawn(), world.spawn(), world.spawn());
        world.send(a, &Hit(1));
        world.send(b, &Hit(2));
        world.send(c, &Hit(3));

        let mut inbox = world.inbox::<Hit>();
        assert_eq!(inbox.next(), Some((a, Hit(1))));
        world.send(a, &Hit(4));
        drop(inbox);

        // unread first, then what was sent meanwhile
        assert_eq!(read(&world), [(b, Hit(2)), (c, Hit(3)), (a, Hit(4))]);
    }

    #[test]
    fn inbox_kept_at_end_of_tick() {
        let mut world = world();
        let a = world.spawn();
        world.send(a, &Hit(1));
        world.end_tick();
        assert_eq!(read(&world), [(a, Hit(1))]);
    }

    #[test]
    fn send_to_replica_goes_to_outbox() {
        let world = world();
        let owned = world.spawn();
        let uid = Uid::new(NonZeroU32::new(9).unwrap(), 4);
        let replica = world.adopt(uid).unwrap();
        world.entities().set_replica(replica, true);

        world.send(replica, &Hit(1));
        world.send(owned, &Hit(2));
        world.send(replica, &Hit(3));

        assert_eq!(read(&world), [(owned, Hit(2))]);
        assert_eq!(world.take_outbox(), [(uid, Hit(1)), (uid, Hit(3))]);
        assert!(world.take_outbox::<Hit>().is_empty());
    }

    #[test]
    fn deliver_pushes_onto_the_inbox() {
        let world = world();
        let a = world.spawn();
        world.send(a, &Hit(1));
        world.deliver(a, Hit(2));
        assert_eq!(read(&world), [(a, Hit(1)), (a, Hit(2))]);
    }

    #[test]
    fn inbox_systems_in_the_schedule() {
        let world = world();
        let (a, b) = (world.spawn(), world.spawn());
        let healed = Rc::new(RefCell::new(Vec::new()));
        let heals = healed.clone();
        let mut schedule = Schedule::new();
        schedule
            .system(Update, move |world: &World| world.send(a, &Hit(10)))
            .system(Inbox, move |world: &World| {
                for (_, hit) in world.inbox::<Hit>() {
                    // reflected: read on the next run
                    world.send(b, &Hit(hit.0 + 1));
                    // read by a later system of this run
                    world.send(a, &Heal(hit.0));
                }
            })
            .system(Inbox, move |world: &World| {
                heals.borrow_mut().extend(world.inbox::<Heal>());
            });

        schedule.run(&world);
        assert_eq!(healed.take(), [(a, Heal(10))]);
        assert_eq!(world.mailbox_mut::<Hit>().inbox, [(b, Hit(11))]);

        schedule.run(&world);
        assert_eq!(healed.take(), [(a, Heal(11)), (a, Heal(10))]);
        assert_eq!(read(&world), [(b, Hit(12)), (b, Hit(11))]);
    }
}
