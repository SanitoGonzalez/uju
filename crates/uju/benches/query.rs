use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use uju::ecs::component::{Replace, Replicated, Visibility};
use uju::ecs::storage::join::join;
use uju::ecs::storage::replace::ReplaceTable;
use uju::ecs::storage::tracked::Tracked;
use uju::net::wire::{Decode, DecodeError, DecodeErrorKind, Encode, EncodeError, en};
use uju::prelude::{Changed, Component, Mut, Query, With, Without, World};

const ENTITIES: usize = 100_000;

#[derive(Component)]
struct Position([f32; 3]);

#[derive(Component)]
struct Velocity([f32; 3]);

#[derive(Component)]
struct Accel([f32; 3]);

#[derive(Component)]
struct Frozen;

/// `Position` in a [`Tracked`] table, which marks on write.
#[derive(Component)]
#[component(storage = Tracked<Self>)]
struct TrackedPosition([f32; 3]);

/// `Position` in a [`ReplaceTable`], which also skips replicas.
#[derive(Component)]
#[component(storage = ReplaceTable<Self>)]
struct ReplacePosition([f32; 3]);

impl Replicated for ReplacePosition {
    const WIRE_ID: u16 = 1;
    const VISIBILITY: Visibility = Visibility::Public;
}

impl Replace for ReplacePosition {
    const SIZE: Option<usize> = Some(12);
}

impl Encode for ReplacePosition {
    const MAX_ENCODED_SIZE: Option<usize> = Some(12);

    fn encoded_size(&self) -> usize {
        12
    }

    fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
        en::encode(self, buf, "ReplacePosition")
    }

    fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
        for value in self.0 {
            w.put(&value.to_le_bytes())?;
        }
        Ok(())
    }
}

impl Decode<'_> for ReplacePosition {
    fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let size = || DecodeError::new(0, "ReplacePosition", DecodeErrorKind::Size);
        let buf: &[u8; 12] = buf.try_into().map_err(|_| size())?;
        let value = |at: usize| f32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
        Ok(Self([value(0), value(4), value(8)]))
    }
}

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
        world
            .view_mut::<TrackedPosition>()
            .insert(entity, TrackedPosition([index as f32; 3]));
        world
            .view_mut::<ReplacePosition>()
            .insert(entity, ReplacePosition([index as f32; 3]));
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

    // every entity is written, so every entry is marked
    c.bench_function("tracked/mut_single", |b| {
        let mut query = Query::<Mut<TrackedPosition>>::new(&world);
        b.iter(|| {
            for (_, mut position) in query.iter() {
                position.0[1] += 1.0;
            }
        })
    });

    c.bench_function("replace/mut_single", |b| {
        let mut query = Query::<Mut<ReplacePosition>>::new(&world);
        b.iter(|| {
            for (_, mut position) in query.iter() {
                position.0[1] += 1.0;
            }
        })
    });

    // reading through the mutable item marks nothing
    c.bench_function("tracked/mut_read", |b| {
        let mut query = Query::<Mut<TrackedPosition>>::new(&world);
        b.iter(|| {
            let mut sum = 0.0;
            for (_, position) in query.iter() {
                sum += position.0[0];
            }
            black_box(sum)
        })
    });

    c.bench_function("tracked/mut_join2", |b| {
        let mut query = Query::<(Mut<TrackedPosition>, Velocity)>::new(&world);
        b.iter(|| {
            for (_, (mut position, velocity)) in query.iter() {
                add(&mut position.0, &velocity.0);
            }
        })
    });

    c.bench_function("replace/mut_join2", |b| {
        let mut query = Query::<(Mut<ReplacePosition>, Velocity)>::new(&world);
        b.iter(|| {
            for (_, (mut position, velocity)) in query.iter() {
                add(&mut position.0, &velocity.0);
            }
        })
    });

    c.bench_function("tracked/changed", |b| {
        let mut query = Query::<(TrackedPosition, Changed<TrackedPosition>)>::new(&world);
        b.iter(|| {
            let mut sum = 0.0;
            for (_, position) in query.iter() {
                sum += position.0[0];
            }
            black_box(sum)
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
