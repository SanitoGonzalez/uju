pub mod cell;
pub mod commands;
pub mod component;
pub mod entity;
mod hash;
pub mod query;
pub mod storage;
pub mod unique;
pub mod world;

pub use uju_macros::*;

pub mod prelude {
    pub use uju_macros::{Component, Unique};

    pub use super::cell::Cell;
    pub use super::entity::{Entity, Uid};
    pub use super::query::{Mut, Owned, Query, Replica, With, Without};
    pub use super::world::World;
}

pub(crate) fn init() {
    component::init();
    unique::init();
}
