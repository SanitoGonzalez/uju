// the components' values are never read, only dropped
#![allow(dead_code)]

use criterion::{Criterion, criterion_group, criterion_main};
use uju::prelude::{Component, World};

const ENTITIES: usize = 10_000;

#[derive(Component)]
struct Position([f32; 3]);

#[derive(Component)]
struct Velocity([f32; 3]);

#[derive(Component)]
struct Hp(u32);

#[derive(Component)]
struct Frozen;

#[derive(Component)]
#[component(require(Hp = Hp(100), Frozen = Frozen))]
struct Enemy;

#[derive(Component)]
struct Ally;

fn bench(c: &mut Criterion) {
    uju::init();

    c.bench_function("spawn/view_inserts", |b| {
        b.iter(|| {
            let world = World::new();
            for index in 0..ENTITIES {
                let entity = world.spawn();
                world
                    .view_mut::<Position>()
                    .insert(entity, Position([index as f32; 3]));
                world
                    .view_mut::<Velocity>()
                    .insert(entity, Velocity([1.0; 3]));
                world.view_mut::<Hp>().insert(entity, Hp(100));
                world.view_mut::<Frozen>().insert(entity, Frozen);
                world.view_mut::<Ally>().insert(entity, Ally);
            }
            world
        })
    });

    c.bench_function("spawn/spawn_with", |b| {
        b.iter(|| {
            let world = World::new();
            for index in 0..ENTITIES {
                world.spawn_with((
                    Position([index as f32; 3]),
                    Velocity([1.0; 3]),
                    Hp(100),
                    Frozen,
                    Ally,
                ));
            }
            world
        })
    });

    c.bench_function("spawn/spawn_with_required", |b| {
        b.iter(|| {
            let world = World::new();
            for index in 0..ENTITIES {
                world.spawn_with((Position([index as f32; 3]), Velocity([1.0; 3]), Enemy));
            }
            world
        })
    });

    c.bench_function("spawn/commands", |b| {
        b.iter(|| {
            let world = World::new();
            for index in 0..ENTITIES {
                let entity = world.spawn();
                world.commands().insert(
                    entity,
                    (
                        Position([index as f32; 3]),
                        Velocity([1.0; 3]),
                        Hp(100),
                        Frozen,
                    ),
                );
            }
            world.flush();
            world
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
