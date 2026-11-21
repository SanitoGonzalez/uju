use std::fmt;

use crate::ecs::Component;

/// An entity's name for debugging, which
/// [`World::inspect`](crate::ecs::world::World::inspect) shows first. It is
/// not in the schema and does not travel.
#[derive(Component, Clone, PartialEq, Eq, Hash, Default, Debug)]
pub struct Name(pub String);

impl Name {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
