use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Once;

use linkme::distributed_slice;

use crate::ecs::storage::audience::{AudienceTable, KeyTable};
use crate::net::wire::{DecodeError, EncodeError};

pub type Id = u16;

/// An audience declared in the schema, as `audience Faction: FactionId`:
/// who sees the components with its visibility nearby, by a key per entity
/// and a set of keys per observer.
///
/// The type is a marker. Its keys live in an internal table of the
/// [`World`](crate::ecs::world::World), which
/// [`set_audience_key`](crate::ecs::world::World::set_audience_key) writes.
/// Audiences are not components, so they get no query term.
pub trait Audience: 'static {
    /// The key: fixed-size, with no floats. `Debug` formats it for
    /// [`World::inspect`](crate::ecs::world::World::inspect).
    type Key: Copy + Eq + Hash + Debug + 'static;

    /// The audience id, dense from `1` over every audience by qualified
    /// name, which names it on the wire.
    const WIRE_ID: u16;

    /// The size of every key's encoding.
    const KEY_SIZE: usize;

    /// The index of its table in a `World`.
    fn id() -> Id;

    /// Writes `key`'s encoding to the start of `buf`, which holds at least
    /// [`KEY_SIZE`](Audience::KEY_SIZE) bytes, and returns its length.
    fn encode_key(key: &Self::Key, buf: &mut [u8]) -> Result<usize, EncodeError>;

    /// Decodes a key from exactly [`KEY_SIZE`](Audience::KEY_SIZE) bytes.
    fn decode_key(buf: &[u8]) -> Result<Self::Key, DecodeError>;
}

pub struct Registration {
    pub name: &'static str,
    pub id: UnsafeCell<u16>,
    pub new_table: fn() -> Box<dyn KeyTable>,
}

unsafe impl Sync for Registration {}

/// Builds `A`'s empty table, for [`Registration::new_table`].
pub fn new_table<A: Audience>() -> Box<dyn KeyTable> {
    Box::new(AudienceTable::<A>::new())
}

#[distributed_slice]
pub static AUDIENCES: [Registration];

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        let registrations = registrations();
        assert!(registrations.len() <= u16::MAX as usize);
        for pair in registrations.windows(2) {
            assert_ne!(pair[0].name, pair[1].name, "duplicate audience name");
        }
        for (index, registration) in registrations.iter().enumerate() {
            unsafe { *registration.id.get() = index as u16 }
        }
    });
}

pub(crate) fn registrations() -> Vec<&'static Registration> {
    let mut registrations: Vec<_> = AUDIENCES.iter().collect();
    registrations.sort_unstable_by_key(|registration| registration.name);
    registrations
}
