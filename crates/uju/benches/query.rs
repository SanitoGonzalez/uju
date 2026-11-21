use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use uju::ecs::component::{Patch, Replace, Replicated, Valued, Visibility};
use uju::ecs::storage::join::join;
use uju::ecs::storage::patch::{FieldMask, PatchTable};
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

impl Valued for ReplacePosition {
    const SIZE: Option<usize> = Some(12);
}

impl Replace for ReplacePosition {}

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

/// `Position` in a [`PatchTable`], with a field per axis.
#[derive(Component)]
#[component(storage = PatchTable<Self>)]
struct PatchPosition {
    x: f32,
    y: f32,
    z: f32,
}

impl Replicated for PatchPosition {
    const WIRE_ID: u16 = 2;
    const VISIBILITY: Visibility = Visibility::Public;
}

impl Valued for PatchPosition {
    const SIZE: Option<usize> = Some(12);
}

impl Encode for PatchPosition {
    const MAX_ENCODED_SIZE: Option<usize> = Some(12);

    fn encoded_size(&self) -> usize {
        12
    }

    fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
        en::encode(self, buf, "PatchPosition")
    }

    fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
        for value in [self.x, self.y, self.z] {
            w.put(&value.to_le_bytes())?;
        }
        Ok(())
    }
}

impl Decode<'_> for PatchPosition {
    fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let [x, y, z] = ReplacePosition::decode(buf)?.0;
        Ok(Self { x, y, z })
    }
}

/// A patch's sent axes.
struct PositionPatch([Option<f32>; 3]);

impl Decode<'_> for PositionPatch {
    fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let error = |kind| DecodeError::new(0, "PositionPatch", kind);
        let (&mask, mut rest) = buf.split_first().ok_or(error(DecodeErrorKind::Size))?;
        if mask > 0b111 {
            return Err(error(DecodeErrorKind::Padding));
        }
        let mut axes = [None; 3];
        for (axis, value) in axes.iter_mut().enumerate() {
            if mask & 1 << axis != 0 {
                let (bytes, tail) = rest
                    .split_first_chunk::<4>()
                    .ok_or(error(DecodeErrorKind::Size))?;
                *value = Some(f32::from_le_bytes(*bytes));
                rest = tail;
            }
        }
        if !rest.is_empty() {
            return Err(error(DecodeErrorKind::Size));
        }
        Ok(PositionPatch(axes))
    }
}

impl Patch for PatchPosition {
    const FIELDS: usize = 3;
    const MAX_PATCH_SIZE: Option<usize> = Some(13);

    type Mut<'a> = PatchPositionMut<'a>;
    type Patch<'a> = PositionPatch;

    fn new_mut<'a>(value: &'a mut Self, mask: FieldMask<'a, Self>) -> PatchPositionMut<'a> {
        PatchPositionMut { value, mask }
    }

    fn patch_size(&self, mask: &[u8]) -> usize {
        1 + 4 * mask[0].count_ones() as usize
    }

    fn encode_patch(&self, mask: &[u8], buf: &mut [u8]) -> Result<usize, EncodeError> {
        let mut w = en::Writer::new(buf, "PositionPatch");
        w.put(mask)?;
        for (axis, value) in [self.x, self.y, self.z].into_iter().enumerate() {
            if mask[0] & 1 << axis != 0 {
                w.put(&value.to_le_bytes())?;
            }
        }
        Ok(w.pos())
    }

    fn apply(&mut self, patch: PositionPatch) {
        let [x, y, z] = patch.0;
        self.x = x.unwrap_or(self.x);
        self.y = y.unwrap_or(self.y);
        self.z = z.unwrap_or(self.z);
    }
}

struct PatchPositionMut<'a> {
    value: &'a mut PatchPosition,
    mask: FieldMask<'a, PatchPosition>,
}

impl PatchPositionMut<'_> {
    fn x(&self) -> f32 {
        self.value.x
    }

    fn y_mut(&mut self) -> &mut f32 {
        self.mask.mark(1);
        &mut self.value.y
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
        world.view_mut::<PatchPosition>().insert(
            entity,
            PatchPosition {
                x: index as f32,
                y: index as f32,
                z: index as f32,
            },
        );
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

    // marks one field of every entry
    c.bench_function("patch/mut_single", |b| {
        let mut query = Query::<Mut<PatchPosition>>::new(&world);
        b.iter(|| {
            for (_, mut position) in query.iter() {
                *position.y_mut() += 1.0;
            }
        })
    });

    c.bench_function("patch/mut_read", |b| {
        let mut query = Query::<Mut<PatchPosition>>::new(&world);
        b.iter(|| {
            let mut sum = 0.0;
            for (_, position) in query.iter() {
                sum += position.x();
            }
            black_box(sum)
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

    c.bench_function("query/option", |b| {
        let mut query = Query::<(Mut<Position>, Option<Velocity>)>::new(&world);
        b.iter(|| {
            for (_, (position, velocity)) in query.iter() {
                if let Some(velocity) = velocity {
                    add(&mut position.0, &velocity.0);
                }
            }
        })
    });

    // no term drives, so the query lists every live entity first
    c.bench_function("query/alive", |b| {
        let mut query = Query::<Without<Frozen>>::new(&world);
        b.iter(|| black_box(query.iter().count()))
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
