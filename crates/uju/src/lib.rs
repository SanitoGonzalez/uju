extern crate self as uju;

pub mod ecs;
pub mod mesh;
pub mod net;
pub mod util;

#[doc(hidden)]
pub use linkme;

pub fn init() {
    ecs::init();
}
