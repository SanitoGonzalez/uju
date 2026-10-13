use uju::ecs::prelude::*;

/// Value of `kind` held by `entity`.
pub fn get(kind: AnyKind, world: &World, entity: Entity) -> Option<f32> {
    match kind {
        AnyKind::Core(kind) => world
            .view::<CoreStats>()
            .get(entity)
            .map(|stats| stats.get(kind)),
    }
}

#[derive(Clone, Copy)]
pub enum AnyKind {
    Core(CoreKind),
}

#[derive(Clone, Copy)]
pub enum CoreKind {
    Strength,
    Intelligence,
    Dexterity,
}

#[derive(Component)]
pub struct CoreStats {
    pub strength: f32,
    pub intelligence: f32,
    pub dexterity: f32,
}

impl CoreStats {
    pub fn get(&self, kind: CoreKind) -> f32 {
        match kind {
            CoreKind::Strength => self.strength,
            CoreKind::Intelligence => self.intelligence,
            CoreKind::Dexterity => self.dexterity,
        }
    }
}
