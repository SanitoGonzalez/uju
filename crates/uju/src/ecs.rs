pub mod component;
pub mod entity;
pub mod query;
pub mod storage;
pub mod unique;
pub mod world;

pub use uju_macros_ecs::*;

pub mod prelude {
    pub use uju_macros_ecs::{Component, Unique};

    pub use super::entity::Entity;
    pub use super::query::{Mut, Query, With, Without};
    pub use super::world::World;
}

pub(crate) fn init() {
    component::init();
    unique::init();
}
