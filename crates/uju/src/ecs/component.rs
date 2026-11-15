use std::cell::UnsafeCell;
use std::sync::Once;

use linkme::distributed_slice;

use crate::ecs::storage::{Storage, patch::FieldMask, table::Table};
use crate::net::wire::{Decode, Encode, EncodeError};

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
///
/// A tag implements only this trait, and is stored in a
/// [`TagTable`](crate::ecs::storage::tag::TagTable). A component with a
/// value also implements [`Valued`] and its mode's trait.
pub trait Replicated: Component {
    /// The id in the component id space, which names it on the wire.
    const WIRE_ID: u16;

    const VISIBILITY: Visibility;

    /// Whether neighbor shards replicating the entity's cell receive it.
    const HALO: bool = !matches!(Self::VISIBILITY, Visibility::None);
}

/// A [`Replicated`] component with a value, as every one but a tag has: its
/// full encoding, which entering entities, snapshots and resyncs send
/// whatever the mode.
///
/// Decoding yields the owned value, so the type implements `Decode` for
/// every lifetime.
pub trait Valued: Replicated + Encode + for<'a> Decode<'a> {
    /// The size of every encoding, when they all have one. A section packs
    /// the values of such a component with no length prefix.
    const SIZE: Option<usize>;
}

/// A component whose changes travel as its whole value, stored in a
/// [`ReplaceTable`](crate::ecs::storage::replace::ReplaceTable).
pub trait Replace: Valued {}

/// A component whose changes travel as patches of the top-level fields
/// written this tick, stored in a
/// [`PatchTable`](crate::ecs::storage::patch::PatchTable).
///
/// The table keeps a mask per entry, a bit per field in declaration order,
/// and hands out [`Mut`](Patch::Mut) wrappers that set the bits. The patch
/// is encoded from the mask and the current value, with no copy of the last
/// value sent.
pub trait Patch: Valued {
    /// The number of top-level fields. A mask takes `FIELDS.div_ceil(8)`
    /// bytes, and its bits past `FIELDS` stay zero.
    const FIELDS: usize;

    /// The largest encoding of a patch, `None` when only
    /// [`MAX_SIZE`](crate::net::wire::MAX_SIZE) bounds it.
    const MAX_PATCH_SIZE: Option<usize>;

    /// The value borrowed mutably: `*_mut()` accessors that mark their
    /// field through the [`FieldMask`], and read accessors that do not.
    type Mut<'a>;

    /// A received patch, validated by decoding.
    type Patch<'a>: Decode<'a>;

    fn new_mut<'a>(value: &'a mut Self, mask: FieldMask<'a, Self>) -> Self::Mut<'a>;

    /// The exact number of bytes [`encode_patch`](Patch::encode_patch)
    /// writes for `mask`.
    fn patch_size(&self, mask: &[u8]) -> usize;

    /// Writes the patch of the fields set in `mask` to the start of `buf`,
    /// which holds at least [`patch_size`](Patch::patch_size) bytes, and
    /// returns its length. The mask is the patch's dirty bits, as is.
    fn encode_patch(&self, mask: &[u8], buf: &mut [u8]) -> Result<usize, EncodeError>;

    /// Overwrites the fields the patch holds.
    fn apply(&mut self, patch: Self::Patch<'_>);
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
