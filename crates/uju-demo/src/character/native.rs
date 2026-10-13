use uju::ecs::component::transform3::Transform3;
use uju::ecs::prelude::*;

use super::effect::{Context, Role, Value};

pub type Id = u32;

pub struct Def {
    pub name: &'static str,
    pub execute: fn(world: &World, ctx: &mut Context, args: &[Value]),
}

pub static DEFS: &[Def] = &[Def {
    name: "swap_positions",
    execute: swap_positions,
}];

pub fn get(id: Id) -> Option<&'static Def> {
    DEFS.get(id as usize)
}

pub fn resolve(name: &str) -> Option<Id> {
    DEFS.iter()
        .position(|def| def.name == name)
        .map(|i| i as Id)
}

fn swap_positions(world: &World, ctx: &mut Context, _args: &[Value]) {
    let (a, b) = (ctx.get(Role::Source), ctx.get(Role::Target));
    let mut transforms = world.view_mut::<Transform3>();
    let (Some(pa), Some(pb)) = (
        transforms.get(a).map(|t| t.position),
        transforms.get(b).map(|t| t.position),
    ) else {
        return;
    };
    if let Some(t) = transforms.get_mut(a) {
        t.position = pb;
    }
    if let Some(t) = transforms.get_mut(b) {
        t.position = pa;
    }
}
