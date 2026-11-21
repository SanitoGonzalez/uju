//! The world's grid of cells, and the system that places entities in them
//! from their [`Transform`]. See `docs/ecs/entities.md`.

use crate::ecs::Unique;
use crate::ecs::cell::Cell;
use crate::ecs::world::World;
use crate::math::Vector;
use crate::transform::Transform;

/// How the world is cut into [`Cell`]s: squares of side `size` on the ground
/// plane, `x` and `y` in 2D, `x` and `z` in 3D, with cell `(0, 0)` starting
/// at the origin.
///
/// A cell id is its row and column, each a cell coordinate from `-32768`
/// to `32767` biased by `32768`, as `row << 16 | column`. Positions past the
/// last cell of an axis fall in it.
///
/// An entity leaves its cell only once it is more than `margin` past the
/// cell's border, so one standing on the line does not flip every tick.
#[derive(Unique, Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    size: f32,
    margin: f32,
}

impl Grid {
    /// Panics unless `size` is finite and positive, and `margin` finite and
    /// not negative.
    pub fn new(size: f32, margin: f32) -> Self {
        assert!(size.is_finite() && size > 0.0, "cell size must be positive");
        assert!(
            margin.is_finite() && margin >= 0.0,
            "cell margin must not be negative"
        );
        Self { size, margin }
    }

    /// The side of a cell.
    pub fn size(&self) -> f32 {
        self.size
    }

    /// How far past its cell's border an entity must be to leave it.
    pub fn margin(&self) -> f32 {
        self.margin
    }

    /// The cell holding `position`.
    pub fn cell(&self, position: Vector) -> Cell {
        let (x, y) = ground(position);
        let column = coordinate(x / self.size);
        let row = coordinate(y / self.size);
        Cell::new((row as u32) << 16 | column as u32)
    }

    /// The cell of an entity in `current` once it is at `position`: `current`
    /// while `position` is within the margin around it, else the cell
    /// holding `position`.
    pub fn cell_from(&self, current: Cell, position: Vector) -> Cell {
        let cell = self.cell(position);
        if cell == current {
            return cell;
        }

        let (x, y) = ground(position);
        let margin = self.margin / self.size;
        let near = |at: f32, id: u32| {
            let start = (id as u16 ^ BIAS) as i16 as f32;
            at >= start - margin && at < start + 1.0 + margin
        };
        let id = current.id();
        if near(x / self.size, id) && near(y / self.size, id >> 16) {
            current
        } else {
            cell
        }
    }
}

const BIAS: u16 = 1 << 15;

/// A cell coordinate, from a position in cells.
#[inline]
fn coordinate(at: f32) -> u16 {
    // the cast saturates, so the outermost cells take what lies past them
    at.floor() as i16 as u16 ^ BIAS
}

/// The two axes of `position` the grid cuts.
#[cfg(feature = "2d")]
#[inline]
fn ground(position: Vector) -> (f32, f32) {
    (position.x, position.y)
}

/// The two axes of `position` the grid cuts. `y` is up.
#[cfg(feature = "3d")]
#[inline]
fn ground(position: Vector) -> (f32, f32) {
    (position.x, position.z)
}

/// Sets the cell of each owned entity whose [`Transform`] was added or
/// changed this tick, from the world's [`Grid`]. An entity seen for the
/// first time, one with no cell or whose `Transform` was added this tick,
/// takes the cell holding its position and is
/// [promoted](World::promote). Any other keeps its
/// cell while within the grid's margin.
///
/// Replicas are skipped: the runtime sets their cell. Does nothing when no
/// `Grid` is installed.
///
/// Panics if the entity allocator or the cell index is borrowed, or
/// `Transform`'s table borrowed mutably, at the time.
#[cfg_attr(not(test), expect(dead_code, reason = "called by the runtime's tick"))]
pub(crate) fn set_cells(world: &World) {
    let Some(grid) = world.get_unique::<Grid>() else {
        return;
    };
    let registry = world.entities();
    let replicas = registry.replicas();
    let transforms = world.view::<Transform>();
    let entities = transforms.entities();

    for position in transforms.changed_positions() {
        let entity = entities[position];
        if replicas.contains(entity) {
            continue;
        }

        let at = transforms.value_at(position).position;
        let cell = match registry.cell(entity) {
            Some(current) if !transforms.is_added_at(position) => grid.cell_from(current, at),
            _ => {
                world.promote(entity);
                grid.cell(at)
            }
        };
        registry.set_cell(entity, cell);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::ecs::entity::{Entity, UidAllocator};
    use crate::math::Rotation;

    /// A position at `x` along the first ground axis and `y` along the
    /// second.
    #[cfg(feature = "2d")]
    fn at(x: f32, y: f32) -> Vector {
        Vector::new(x, y)
    }

    #[cfg(feature = "3d")]
    fn at(x: f32, y: f32) -> Vector {
        Vector::new(x, 0.0, y)
    }

    #[cfg(any(feature = "2d", feature = "3d-yaw"))]
    fn rotation() -> Rotation {
        Rotation::from(0.0)
    }

    #[cfg(all(feature = "3d", not(feature = "3d-yaw")))]
    fn rotation() -> Rotation {
        Rotation::IDENTITY
    }

    fn transform(position: Vector) -> Transform {
        Transform {
            position,
            rotation: rotation(),
        }
    }

    /// The cell at column `x` and row `y`.
    fn cell(x: i16, y: i16) -> Cell {
        Cell::new(((y as u16 ^ BIAS) as u32) << 16 | (x as u16 ^ BIAS) as u32)
    }

    fn world() -> World {
        crate::ecs::init();
        let world = World::new();
        world.install_uid_allocator(UidAllocator::new(NonZeroU32::MIN, NonZeroU32::MAX));
        world.insert_unique(Grid::new(10.0, 1.0));
        world
    }

    fn spawn_at(world: &World, x: f32, y: f32) -> Entity {
        let entity = world.spawn();
        world
            .view_mut::<Transform>()
            .insert(entity, transform(at(x, y)));
        entity
    }

    fn move_to(world: &World, entity: Entity, x: f32, y: f32) {
        world
            .view_mut::<Transform>()
            .get_mut(entity)
            .unwrap()
            .position = at(x, y);
    }

    #[test]
    fn grid_cell() {
        let grid = Grid::new(10.0, 1.0);
        assert_eq!(grid.cell(at(0.0, 0.0)), cell(0, 0));
        assert_eq!(grid.cell(at(9.9, 0.0)), cell(0, 0));
        assert_eq!(grid.cell(at(10.0, 0.0)), cell(1, 0));
        assert_eq!(grid.cell(at(-0.1, 25.0)), cell(-1, 2));
        assert_eq!(grid.cell(at(-10.0, -10.1)), cell(-1, -2));
        assert_eq!(cell(0, 0).id(), 0x8000_8000);

        // past the last cell of an axis, the outermost one
        assert_eq!(grid.cell(at(1e9, -1e9)), cell(i16::MAX, i16::MIN));
        assert_eq!(grid.cell(at(f32::INFINITY, 0.0)), cell(i16::MAX, 0));
    }

    #[cfg(feature = "2d")]
    #[test]
    fn grid_axes_2d() {
        let grid = Grid::new(10.0, 1.0);
        assert_eq!(grid.cell(Vector::new(15.0, 25.0)), cell(1, 2));
    }

    #[cfg(feature = "3d")]
    #[test]
    fn grid_axes_3d() {
        let grid = Grid::new(10.0, 1.0);
        // y is up and does not cut
        assert_eq!(grid.cell(Vector::new(15.0, 999.0, 25.0)), cell(1, 2));
        assert_eq!(grid.cell(Vector::new(15.0, -999.0, 25.0)), cell(1, 2));
    }

    #[test]
    fn grid_margin() {
        let grid = Grid::new(10.0, 1.0);
        let home = cell(0, 0);

        // inside the margin, the cell holds
        assert_eq!(grid.cell_from(home, at(10.5, 5.0)), home);
        assert_eq!(grid.cell_from(home, at(-0.9, -0.9)), home);
        assert_eq!(grid.cell_from(home, at(10.9, 10.9)), home);

        // past it, on either axis, the cell holding the position
        assert_eq!(grid.cell_from(home, at(11.0, 5.0)), cell(1, 0));
        assert_eq!(grid.cell_from(home, at(5.0, -1.1)), cell(0, -1));
        assert_eq!(grid.cell_from(home, at(30.0, 30.0)), cell(3, 3));

        // the way back crosses the same margin
        let next = cell(1, 0);
        assert_eq!(grid.cell_from(next, at(9.5, 5.0)), next);
        assert_eq!(grid.cell_from(next, at(8.9, 5.0)), home);

        // no margin: the line decides
        let grid = Grid::new(10.0, 0.0);
        assert_eq!(grid.cell_from(home, at(10.0, 5.0)), cell(1, 0));
        assert_eq!(grid.cell_from(cell(1, 0), at(9.99, 5.0)), home);
    }

    #[test]
    #[should_panic(expected = "cell size must be positive")]
    fn grid_zero_size() {
        Grid::new(0.0, 0.0);
    }

    #[test]
    fn set_cells_first_seen() {
        let world = world();
        let entity = spawn_at(&world, 15.0, 25.0);
        assert_eq!(world.cell(entity), None);
        assert_eq!(world.uid(entity), None);

        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(1, 2)));
        assert!(world.uid(entity).is_some());
        assert!(world.entities().cell_index().contains(cell(1, 2), entity));
    }

    #[test]
    fn set_cells_margin() {
        let mut world = world();
        let entity = spawn_at(&world, 9.5, 5.0);
        set_cells(&world);
        let uid = world.uid(entity);
        world.end_tick();

        // on the line, within the margin
        for x in [10.2, 9.8, 10.9, 9.1] {
            move_to(&world, entity, x, 5.0);
            set_cells(&world);
            assert_eq!(world.cell(entity), Some(cell(0, 0)));
            world.end_tick();
        }

        move_to(&world, entity, 11.1, 5.0);
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(1, 0)));
        assert_eq!(world.entities().tick_start_cell(entity), Some(cell(0, 0)));
        assert_eq!(world.uid(entity), uid);
        world.end_tick();

        move_to(&world, entity, 9.5, 5.0);
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(1, 0)));
    }

    #[test]
    fn set_cells_walks_changed_only() {
        let mut world = world();
        let entity = spawn_at(&world, 5.0, 5.0);
        set_cells(&world);
        world.end_tick();

        // a cell set elsewhere stays while the `Transform` is clean
        world.entities().set_cell(entity, cell(7, 7));
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(7, 7)));

        // reading through the mutable item marks nothing
        assert_eq!(
            world
                .view_mut::<Transform>()
                .get_mut(entity)
                .unwrap()
                .position,
            at(5.0, 5.0)
        );
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(7, 7)));
    }

    #[test]
    fn set_cells_readded() {
        let mut world = world();
        let entity = spawn_at(&world, 5.0, 5.0);
        set_cells(&world);
        world.end_tick();

        // losing the `Transform` keeps the cell
        world.view_mut::<Transform>().remove(entity);
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(0, 0)));
        world.end_tick();

        // added again, it takes the cell holding it, margin or not
        world
            .view_mut::<Transform>()
            .insert(entity, transform(at(10.5, 5.0)));
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(1, 0)));
    }

    #[test]
    fn set_cells_skips_replicas() {
        let world = world();
        let entity = world.spawn();
        world.entities().set_replica(entity, true);
        world
            .view_mut::<Transform>()
            .insert(entity, transform(at(5.0, 5.0)));

        set_cells(&world);
        assert_eq!(world.cell(entity), None);
        assert_eq!(world.uid(entity), None);
    }

    #[test]
    fn set_cells_without_grid() {
        crate::ecs::init();
        let world = World::new();
        let entity = spawn_at(&world, 5.0, 5.0);
        set_cells(&world);
        assert_eq!(world.cell(entity), None);
    }

    #[test]
    fn set_cells_without_allocator() {
        crate::ecs::init();
        let world = World::new();
        world.insert_unique(Grid::new(10.0, 1.0));
        let entity = spawn_at(&world, 5.0, 5.0);
        set_cells(&world);
        assert_eq!(world.cell(entity), Some(cell(0, 0)));
        assert_eq!(world.uid(entity), None);
    }
}
