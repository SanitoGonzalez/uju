use uju::ecs::prelude::*;

/// Current amount of `kind` held by `entity`.
pub fn get(kind: Kind, world: &World, entity: Entity) -> Option<f32> {
    match kind {
        Kind::Health => world
            .view::<Health>()
            .get(entity)
            .map(|health| health.current),
        Kind::Mana => world.view::<Mana>().get(entity).map(|mana| mana.current),
    }
}

#[derive(Clone, Copy)]
pub enum Kind {
    Health,
    Mana,
}

#[derive(Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

#[derive(Component)]
pub struct Mana {
    pub current: f32,
    pub max: f32,
}
