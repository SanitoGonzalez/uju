use std::any::Any;
use std::cell::UnsafeCell;
use std::sync::Once;

use linkme::distributed_slice;

pub type Id = u16;

/// A plain Rust type, with no schema, that one system emits and later ones
/// read in the same tick, through
/// [`World::emit`](crate::ecs::world::World::emit) and
/// [`World::events`](crate::ecs::world::World::events).
///
/// Its queue is cleared at the end of the tick, and never leaves the
/// `World`.
pub trait Event: 'static {
    /// The index of its queue in a `World`.
    fn id() -> Id;
}

/// An event type's queue in a `World`, erased so the `World` can clear it.
pub trait Queue: Any {
    fn clear(&mut self);
}

impl<E: Event> Queue for Vec<E> {
    fn clear(&mut self) {
        Vec::clear(self);
    }
}

pub struct Registration {
    pub name: &'static str,
    pub id: UnsafeCell<u16>,
    pub new_queue: fn() -> Box<dyn Queue>,
}

unsafe impl Sync for Registration {}

/// Builds `E`'s empty queue, for [`Registration::new_queue`].
pub fn new_queue<E: Event>() -> Box<dyn Queue> {
    Box::new(Vec::<E>::new())
}

#[distributed_slice]
pub static EVENTS: [Registration];

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        let registrations = registrations();
        assert!(registrations.len() <= u16::MAX as usize);
        for pair in registrations.windows(2) {
            assert_ne!(pair[0].name, pair[1].name, "duplicate event name");
        }
        for (index, registration) in registrations.iter().enumerate() {
            unsafe { *registration.id.get() = index as u16 }
        }
    });
}

pub(crate) fn registrations() -> Vec<&'static Registration> {
    let mut registrations: Vec<_> = EVENTS.iter().collect();
    registrations.sort_unstable_by_key(|registration| registration.name);
    registrations
}

#[cfg(test)]
mod tests {
    use crate::ecs::world::World;

    #[derive(crate::ecs::Event, Debug, PartialEq)]
    struct Died(u32);

    #[derive(crate::ecs::Event, Debug, PartialEq)]
    struct Spotted(u32);

    fn world() -> World {
        crate::ecs::init();
        World::new()
    }

    #[test]
    fn events_in_order() {
        let world = world();
        assert!(world.events::<Died>().is_empty());

        world.emit(Died(1));
        world.emit(Died(2));
        world.emit(Spotted(3));

        // any number of readers at once
        let first = world.events::<Died>();
        let second = world.events::<Died>();
        assert_eq!(*first, [Died(1), Died(2)]);
        assert_eq!(*second, [Died(1), Died(2)]);
        assert_eq!(*world.events::<Spotted>(), [Spotted(3)]);
    }

    #[test]
    fn events_queues_are_borrows_of_their_own() {
        let world = world();
        world.emit(Died(1));
        for died in world.events::<Died>().iter() {
            world.emit(Spotted(died.0));
        }
        assert_eq!(*world.events::<Spotted>(), [Spotted(1)]);
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn events_emit_while_read() {
        let world = world();
        world.emit(Died(1));
        for died in world.events::<Died>().iter() {
            world.emit(Died(died.0 + 1));
        }
    }

    #[test]
    fn events_cleared_at_end_of_tick() {
        let mut world = world();
        world.emit(Died(1));
        world.emit(Spotted(2));
        world.end_tick();
        assert!(world.events::<Died>().is_empty());
        assert!(world.events::<Spotted>().is_empty());

        world.emit(Died(3));
        assert_eq!(*world.events::<Died>(), [Died(3)]);
    }
}
