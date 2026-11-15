use std::cell::UnsafeCell;
use std::sync::Once;

use linkme::distributed_slice;

use crate::ecs::storage::{Storage, table::Table};
use crate::net::wire::{Decode, Encode};

pub type Id = u16;

pub trait Component: 'static {
    /// The table that holds this component. `#[derive(Component)]` picks
    /// [`SparseSet`](crate::ecs::storage::sparse_set::SparseSet), unless
    /// told otherwise with `#[component(storage = Tracked<Self>)]`.
    type Storage: Storage<Component = Self>;

    fn id() -> Id;
}

/// Who receives a replicated component's changes, besides the halo stream.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Visibility {
    /// Every client whose view covers the entity's cell.
    Public,
    /// The entity's own client, through its owner frames.
    Owner,
    /// No client.
    None,
    /// The clients whose observer set holds the entity's key for the
    /// audience with this wire id.
    Audience(u16),
}

/// A component declared in the schema, which travels with its entity: what
/// every component and tag has.
pub trait Replicated: Component {
    /// The id in the component id space, which names it on the wire.
    const WIRE_ID: u16;

    const VISIBILITY: Visibility;

    /// Whether neighbor shards replicating the entity's cell receive it.
    const HALO: bool = !matches!(Self::VISIBILITY, Visibility::None);
}

/// A [`Replicated`] component whose changes travel as its whole value,
/// stored in a [`ReplaceTable`](crate::ecs::storage::replace::ReplaceTable).
///
/// Decoding yields the owned value, so the type implements `Decode` for
/// every lifetime.
pub trait Replace: Replicated + Encode + for<'a> Decode<'a> {
    /// The size of every encoding, when they all have one. A section packs
    /// the values of such a component with no length prefix.
    const SIZE: Option<usize>;
}

/// What the runtime needs to route a replicated table's sections, read
/// through [`Table::replication`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Replication {
    pub wire_id: u16,
    pub visibility: Visibility,
    pub halo: bool,
}

impl Replication {
    pub const fn of<C: Replicated>() -> Self {
        Self {
            wire_id: C::WIRE_ID,
            visibility: C::VISIBILITY,
            halo: C::HALO,
        }
    }
}

pub struct Registration {
    pub name: &'static str,
    pub id: UnsafeCell<u16>,
    pub new_table: fn() -> Box<dyn Table>,
}

unsafe impl Sync for Registration {}

/// Builds `C`'s empty table, for [`Registration::new_table`].
pub fn new_table<C: Component>() -> Box<dyn Table> {
    Box::new(C::Storage::default())
}

#[distributed_slice]
pub static COMPONENTS: [Registration];

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        let registrations = registrations();
        assert!(registrations.len() <= u16::MAX as usize);
        for pair in registrations.windows(2) {
            assert_ne!(pair[0].name, pair[1].name, "duplicate component name");
        }
        for (index, registration) in registrations.iter().enumerate() {
            unsafe { *registration.id.get() = index as u16 }
        }
    });
}

pub(crate) fn registrations() -> Vec<&'static Registration> {
    let mut registrations: Vec<_> = COMPONENTS.iter().collect();
    registrations.sort_unstable_by_key(|registration| registration.name);
    registrations
}
