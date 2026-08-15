pub mod transform2;
pub mod transform3;
pub mod velocity2;
pub mod velocity3;

use std::cell::UnsafeCell;
use std::sync::Once;

use linkme::distributed_slice;

use crate::ecs::storage::table::Table;

pub type Id = u16;

pub trait Component: Send + 'static {
    fn id() -> Id;
}

pub struct Registration {
    pub name: &'static str,
    pub id: UnsafeCell<u16>,
    pub new_table: fn() -> Box<dyn Table>,
}

unsafe impl Sync for Registration {}

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
