pub mod time;
pub mod view;

use std::cell::UnsafeCell;
use std::sync::Once;

use linkme::distributed_slice;

pub type Id = u16;

pub trait Unique: Send + 'static {
    fn id() -> Id;
}

pub struct Registration {
    pub name: &'static str,
    pub id: UnsafeCell<u16>,
}

unsafe impl Sync for Registration {}

#[distributed_slice]
pub static UNIQUES: [Registration];

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        let mut registrations: Vec<_> = UNIQUES.iter().collect();
        registrations.sort_unstable_by_key(|registration| registration.name);
        assert!(registrations.len() <= u16::MAX as usize);
        for pair in registrations.windows(2) {
            assert_ne!(pair[0].name, pair[1].name, "duplicate unique name");
        }
        for (index, registration) in registrations.iter().enumerate() {
            unsafe { *registration.id.get() = index as u16 }
        }
    });
}

pub(crate) fn count() -> usize {
    UNIQUES.len()
}
