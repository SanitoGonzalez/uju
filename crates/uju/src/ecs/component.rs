use std::cell::UnsafeCell;
use std::sync::Once;

use linkme::distributed_slice;

use crate::ecs::storage::{Storage, table::Table};

pub type Id = u16;

pub trait Component: 'static {
    /// The table that holds this component. `#[derive(Component)]` picks
    /// [`SparseSet`](crate::ecs::storage::sparse_set::SparseSet).
    type Storage: Storage<Component = Self>;

    fn id() -> Id;
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
