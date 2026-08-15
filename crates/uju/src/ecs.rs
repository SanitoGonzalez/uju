pub mod component;
pub mod entity;
pub mod ghost;
pub mod storage;
pub mod unique;
pub mod world;

pub use uju_macros_ecs::*;

pub(crate) fn init() {
    component::init();
    unique::init();
}
