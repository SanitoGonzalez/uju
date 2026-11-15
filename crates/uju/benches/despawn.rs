// the components' values are never read, only dropped
#![allow(dead_code)]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use uju::ecs::entity::Entity;
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

/// Every entity has a `Position` and an `Hp`, every 2nd a `Velocity` and
/// every 5th is `Frozen`.
fn world() -> (World, Vec<Entity>) {
    uju::init();
    let world = World::new();
    let entities = (0..ENTITIES)
        .map(|index| {
            let entity = world.spawn();
            world
                .view_mut::<Position>()
                .insert(entity, Position([index as f32; 3]));
            world.view_mut::<Hp>().insert(entity, Hp(100));
            if index % 2 == 0 {
                world
                    .view_mut::<Velocity>()
                    .insert(entity, Velocity([1.0; 3]));
            }
            if index % 5 == 0 {
                world.view_mut::<Frozen>().insert(entity, Frozen);
            }
            entity
        })
        .collect();
    (world, entities)
}

fn bench(c: &mut Criterion) {
    c.bench_function("despawn/one_by_one", |b| {
        b.iter_batched(
            world,
            |(world, entities)| {
                for entity in entities {
                    world.despawn(entity);
                }
                world
            },
            BatchSize::LargeInput,
        )
    });

    c.bench_function("despawn/batch", |b| {
        b.iter_batched(
            world,
            |(world, entities)| {
                world.despawn_batch(&entities);
                world
            },
            BatchSize::LargeInput,
        )
    });

    c.bench_function("despawn/commands", |b| {
        b.iter_batched(
            world,
            |(world, entities)| {
                for &entity in &entities {
                    world.commands().despawn(entity);
                }
                world.flush();
                world
            },
            BatchSize::LargeInput,
        )
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
