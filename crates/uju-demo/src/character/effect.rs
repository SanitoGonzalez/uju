use parry3d::glamx::Vec3;
use rand::RngExt;
use smallvec::SmallVec;
use uju::ecs::component::transform3::Transform3;
use uju::ecs::prelude::*;

use super::{area_effect, resource, stat, status_effect};

pub type VarId = usize;

#[derive(Clone, Copy)]
pub enum Role {
    Origin,
    Source,
    Target,
}

#[derive(Default)]
pub enum Effect {
    #[default]
    Nop,
    Seq(Vec<Effect>),
    If {
        condition: Predicate,
        then: Box<Effect>,
        otherwise: Option<Box<Effect>>,
    },
    ForEach {
        target: Target,
        body: Box<Effect>,
    },
    Repeat {
        count: Value,
        body: Box<Effect>,
    },
    Chance {
        p: Value,
        body: Box<Effect>,
    },
    Let {
        slot: VarId,
        value: Value,
        body: Box<Effect>,
    },
    // Native { id: NativeId, args: Vec<Value> }, // native code execution
    Damage {
        to: Role,
        amount: Value,
        // kind: DamageKind,
    },
    Heal {
        to: Role,
        amount: Value,
    },
    ApplyStatusEffect {
        to: Role,
        id: status_effect::Id,
        stacks: Value,
    },
    SpawnAreaEffect {
        at: Role,
        id: area_effect::Id,
    },
}

pub enum Value {
    Const(f32),
    Var(VarId),
    Stat(Role, stat::AnyKind),
    Resource(Role, resource::Kind),
    Distance(Role, Role),
    Count(Box<Target>),
    StatusEffectStacks(Role, status_effect::Id),

    Sum(Box<Value>, Box<Value>),
    Sub(Box<Value>, Box<Value>),
    Mul(Box<Value>, Box<Value>),
    Div(Box<Value>, Box<Value>),
    Neg(Box<Value>),
    Min(Box<Value>, Box<Value>),
    Max(Box<Value>, Box<Value>),
    Random { min: Box<Value>, max: Box<Value> },
    If(Box<Predicate>, Box<Value>, Box<Value>),
}

impl Value {
    pub fn eval(&self, world: &World, ctx: &mut Context) -> f32 {
        match self {
            Value::Const(value) => *value,
            Value::Var(slot) => {
                debug_assert!(*slot < ctx.vars.len(), "unbound var {slot}");
                ctx.vars.get(*slot).copied().unwrap_or(0.0)
            }
            Value::Stat(role, kind) => stat::get(*kind, world, ctx.get(*role)).unwrap_or(0.0),
            Value::Resource(role, kind) => {
                resource::get(*kind, world, ctx.get(*role)).unwrap_or(0.0)
            }
            Value::Distance(a, b) => {
                let transforms = world.view::<Transform3>();
                match (transforms.get(ctx.get(*a)), transforms.get(ctx.get(*b))) {
                    (Some(a), Some(b)) => a.position.distance(b.position),
                    // unplaced entities are out of any range
                    _ => f32::INFINITY,
                }
            }
            Value::Count(target) => target.count(world, ctx) as f32,
            Value::StatusEffectStacks(role, id) => {
                status_effect::stacks(*id, world, ctx.get(*role)).unwrap_or(0) as f32
            }
            Value::Sum(a, b) => a.eval(world, ctx) + b.eval(world, ctx),
            Value::Sub(a, b) => a.eval(world, ctx) - b.eval(world, ctx),
            Value::Mul(a, b) => a.eval(world, ctx) * b.eval(world, ctx),
            Value::Div(a, b) => {
                let a = a.eval(world, ctx);
                let b = b.eval(world, ctx);
                // safe division: authored formulas shouldn't blow up into inf/NaN
                if b == 0.0 { 0.0 } else { a / b }
            }
            Value::Neg(a) => -a.eval(world, ctx),
            Value::Min(a, b) => a.eval(world, ctx).min(b.eval(world, ctx)),
            Value::Max(a, b) => a.eval(world, ctx).max(b.eval(world, ctx)),
            Value::Random { min, max } => {
                let min = min.eval(world, ctx);
                let max = max.eval(world, ctx);
                if min < max {
                    rand::rng().random_range(min..max)
                } else {
                    min
                }
            }
            Value::If(condition, then, otherwise) => {
                if condition.eval(world, ctx) {
                    then.eval(world, ctx)
                } else {
                    otherwise.eval(world, ctx)
                }
            }
        }
    }
}

pub enum Predicate {
    True,
    Not(Box<Predicate>),
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Lt(Value, Value),
    Gt(Value, Value),
    // HasStatus(Role, StatusId),
    // IsHostile(Role, Role),
    IsSame(Role, Role),
}

impl Predicate {
    pub fn eval(&self, world: &World, ctx: &mut Context) -> bool {
        match self {
            Predicate::True => true,
            Predicate::Not(p) => !p.eval(world, ctx),
            Predicate::And(ps) => ps.iter().all(|p| p.eval(world, ctx)),
            Predicate::Or(ps) => ps.iter().any(|p| p.eval(world, ctx)),
            Predicate::Lt(a, b) => a.eval(world, ctx) < b.eval(world, ctx),
            Predicate::Gt(a, b) => a.eval(world, ctx) > b.eval(world, ctx),
            Predicate::IsSame(a, b) => ctx.get(*a) == ctx.get(*b),
        }
    }
}

pub type Targets = SmallVec<[Entity; 8]>;

pub enum Target {
    Role(Role),
    InRadius {
        center: Role,
        radius: Value,
        filter: Predicate,
    },
    Nearest {
        center: Role,
        count: Value,
        filter: Predicate,
    },
    Farthest {
        center: Role,
        radius: Value,
        count: Value,
        filter: Predicate,
    },
}

impl Target {
    pub fn count(&self, world: &World, ctx: &mut Context) -> usize {
        match self {
            Target::Role(_) => 1,
            _ => todo!(),
        }
    }

    /// Resolves the entities this target selects. Area selections evaluate
    /// `filter` with each candidate bound to [`Role::Target`].
    pub fn resolve(&self, world: &World, ctx: &mut Context) -> Targets {
        match self {
            Target::Role(role) => {
                let entity = ctx.get(*role);
                if world.is_alive(entity) {
                    Targets::from_elem(entity, 1)
                } else {
                    Targets::new()
                }
            }
            Target::InRadius {
                center,
                radius,
                filter,
            } => {
                let radius = radius.eval(world, ctx);
                Self::candidates(world, ctx, *center, radius, filter)
                    .into_iter()
                    .map(|(entity, _)| entity)
                    .collect()
            }
            Target::Nearest {
                center,
                count,
                filter,
            } => {
                let count = count.eval(world, ctx) as usize;
                let mut candidates = Self::candidates(world, ctx, *center, f32::INFINITY, filter);
                candidates.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
                candidates
                    .into_iter()
                    .take(count)
                    .map(|(entity, _)| entity)
                    .collect()
            }
            Target::Farthest {
                center,
                radius,
                count,
                filter,
            } => {
                let radius = radius.eval(world, ctx);
                let count = count.eval(world, ctx) as usize;
                let mut candidates = Self::candidates(world, ctx, *center, radius, filter);
                candidates.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
                candidates
                    .into_iter()
                    .take(count)
                    .map(|(entity, _)| entity)
                    .collect()
            }
        }
    }

    /// Placed entities within `radius` of `center` passing `filter`, paired
    /// with their squared distance.
    fn candidates(
        world: &World,
        ctx: &mut Context,
        center: Role,
        radius: f32,
        filter: &Predicate,
    ) -> SmallVec<[(Entity, f32); 8]> {
        // todo: use spatial hash for optimization
        let mut candidates = SmallVec::new();
        if radius.is_nan() || radius < 0.0 {
            return candidates;
        }
        let transforms = world.view::<Transform3>();
        let Some(origin) = transforms.get(ctx.get(center)).map(|t| t.position) else {
            return candidates;
        };
        let radius_sq = radius * radius;
        let positions: SmallVec<[(Entity, Vec3); 16]> = transforms
            .iter()
            .map(|(entity, transform)| (entity, transform.position))
            .collect();
        // release the borrow before evaluating `filter`
        drop(transforms);

        let target = ctx.target;
        for (entity, position) in positions {
            let distance_sq = origin.distance_squared(position);
            if distance_sq > radius_sq {
                continue;
            }
            ctx.target = entity;
            let pass = filter.eval(world, ctx);
            ctx.target = target;
            if pass {
                candidates.push((entity, distance_sq));
            }
        }
        candidates
    }
}

pub struct Context {
    pub origin: Entity,
    pub source: Entity,
    pub target: Entity,
    pub vars: SmallVec<[f32; 4]>,
    pub depth: u8,
}

impl Context {
    pub fn new(origin: Entity, source: Entity, target: Entity) -> Self {
        Self {
            origin,
            source,
            target,
            vars: SmallVec::new(),
            depth: 0,
        }
    }

    pub fn get(&self, role: Role) -> Entity {
        match role {
            Role::Origin => self.origin,
            Role::Source => self.source,
            Role::Target => self.target,
        }
    }
}
