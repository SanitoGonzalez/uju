pub mod audience;
pub mod bundle;
pub mod cell;
pub mod commands;
pub mod component;
pub mod entity;
pub mod event;
mod hash;
pub mod hook;
pub mod message;
pub mod query;
pub mod schedule;
pub mod storage;
pub mod unique;
pub mod world;

pub use uju_macros::*;

pub mod prelude {
    pub use uju_macros::{Component, Event, Unique};

    pub use super::cell::Cell;
    pub use super::entity::{Entity, Uid};
    pub use super::query::{Added, Changed, Has, Mut, Or, Owned, Query, Replica, With, Without};
    pub use super::schedule::{Inbox, IntoSystems, Schedule, Update, every};
    pub use super::world::World;
}

pub(crate) fn init() {
    audience::init();
    component::init();
    event::init();
    message::init();
    unique::init();
}
