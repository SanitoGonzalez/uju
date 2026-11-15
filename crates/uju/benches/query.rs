use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use uju::ecs::storage::join::join;
use uju::prelude::{Component, Mut, Query, With, Without, World};

const ENTITIES: usize = 100_000;

#[derive(Component)]
struct Position([f32; 3]);

#[derive(Component)]
struct Velocity([f32; 3]);

#[derive(Component)]
struct Accel([f32; 3]);

#[derive(Component)]
struct Frozen;

/// Every entity has a `Position`, every 2nd a `Velocity`, every 3rd an
/// `Accel` and every 5th is `Frozen`, so terms overlap only in part.
fn world() -> World {
    uju::init();
    let world = World::new();
    for index in 0..ENTITIES {
        let entity = world.spawn();
        world
            .view_mut::<Position>()
            .insert(entity, Position([index as f32; 3]));
        if index % 2 == 0 {
            world
                .view_mut::<Velocity>()
                .insert(entity, Velocity([1.0; 3]));
        }
        if index % 3 == 0 {
            world.view_mut::<Accel>().insert(entity, Accel([0.5; 3]));
        }
        if index % 5 == 0 {
            world.view_mut::<Frozen>().insert(entity, Frozen);
        }
    }
    world
}

fn add(a: &mut [f32; 3], b: &[f32; 3]) {
    for (a, b) in a.iter_mut().zip(b) {
        *a += b;
    }
}

fn bench(c: &mut Criterion) {
    let world = world();

    c.bench_function("query/single", |b| {
        let mut query = Query::<Position>::new(&world);
        b.iter(|| {
            let mut sum = 0.0;
            for (_, position) in query.iter() {
                sum += position.0[0];
            }
            black_box(sum)
        })
    });

    c.bench_function("query/mut_single", |b| {
        let mut query = Query::<Mut<Position>>::new(&world);
        b.iter(|| {
            for (_, position) in query.iter() {
                position.0[1] += 1.0;
            }
        })
    });

    c.bench_function("query/mut_join2", |b| {
        let mut query = Query::<(Mut<Position>, Velocity)>::new(&world);
        b.iter(|| {
            for (_, (position, velocity)) in query.iter() {
                add(&mut position.0, &velocity.0);
            }
        })
    });

    c.bench_function("query/mut_join3", |b| {
        let mut query = Query::<(Mut<Velocity>, Accel, Position)>::new(&world);
        b.iter(|| {
            for (_, (velocity, accel, position)) in query.iter() {
                add(&mut velocity.0, &accel.0);
                black_box(position);
            }
        })
    });

    c.bench_function("query/filters", |b| {
        let mut query =
            Query::<(Mut<Position>, Velocity, With<Accel>, Without<Frozen>)>::new(&world);
        b.iter(|| {
            for (_, (position, velocity)) in query.iter() {
                add(&mut position.0, &velocity.0);
            }
        })
    });

    c.bench_function("join/views_mut2", |b| {
        let mut positions = world.view_mut::<Position>();
        let velocities = world.view::<Velocity>();
        b.iter(|| {
            for (_, (position, velocity)) in join((&mut positions, &velocities)) {
                add(&mut position.0, &velocity.0);
            }
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
