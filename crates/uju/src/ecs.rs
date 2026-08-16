pub mod component;
pub mod entity;
pub mod replica;
pub mod storage;
pub mod tx;
pub mod unique;
pub mod world;

pub use uju_macros_ecs::*;

pub(crate) fn init() {
    component::init();
    unique::init();
}
