use uju::ecs::prelude::*;

use super::effect::Effect;

pub type Id = u32;

pub enum Stacking {
    Refresh,
    Add,
    Independent,
}

pub struct Def {
    pub id: Id,
    pub stacking: Stacking,
    pub max_stacks: u32,
    pub max_ticks: u32,
    pub on_apply: Effect,
    pub on_tick: Effect,
    pub on_expire: Effect,
}

pub struct State {
    pub id: Id,
    pub stacks: u32,
    pub ticks: u32,
}

impl State {
    pub fn new(id: Id) -> Self {
        Self {
            id,
            stacks: 1,
            ticks: 0,
        }
    }
}

#[derive(Component)]
pub struct StatusEffects {
    pub active: Vec<State>,
}

/// Number of active stacks of `id` on `entity`.
pub fn stacks(id: Id, world: &World, entity: Entity) -> Option<u32> {
    world
        .view::<StatusEffects>()
        .get(entity)
        .and_then(|effects| {
            effects
                .active
                .iter()
                .find(|state| state.id == id)
                .map(|state| state.stacks)
        })
}
