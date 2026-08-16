use std::cell::{Ref, RefMut};
use std::ops::{Deref, DerefMut};

use crate::ecs::unique::Unique;

/// Shared borrow of a unique, checked out of the [`World`].
///
/// Derefs to `U`. The borrow is dynamic (`RefCell`), so it must be dropped
/// before the same unique is checked out mutably.
///
/// [`World`]: crate::ecs::world::World
pub struct Uni<'w, U: Unique> {
    pub(crate) guard: Ref<'w, U>,
}

/// Exclusive borrow of a unique, checked out of the [`World`].
///
/// [`World`]: crate::ecs::world::World
pub struct UniMut<'w, U: Unique> {
    pub(crate) guard: RefMut<'w, U>,
}

impl<U: Unique> Deref for Uni<'_, U> {
    type Target = U;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<U: Unique> Deref for UniMut<'_, U> {
    type Target = U;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<U: Unique> DerefMut for UniMut<'_, U> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}
