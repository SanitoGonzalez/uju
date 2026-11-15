//! The section of a cell frame that one replicated table writes and reads:
//! `component:u16 | parts:u8 | part*`, each part `length:u32 | item*`.

use std::any::type_name;

use crate::ecs::component::{Replicated, Valued};
use crate::ecs::entity::Uid;
use crate::net::wire::{DecodeError, DecodeErrorKind, EncodeError, EncodeErrorKind};

/// The part holding full values.
pub(crate) const VALUES: u8 = 1 << 0;
/// The part holding patches or ops.
pub(crate) const CHANGES: u8 = 1 << 1;
/// The part holding removed uids.
pub(crate) const REMOVED: u8 = 1 << 2;

/// Appends `T`'s section with the parts `write` adds, in bit order. Writes
/// nothing and returns `false` when no part has items, and nothing on
/// error.
pub(crate) fn encode<T: Replicated>(
    out: &mut Vec<u8>,
    write: impl FnOnce(&mut Parts<'_>) -> Result<(), EncodeError>,
) -> Result<bool, EncodeError> {
    let start = out.len();
    out.extend_from_slice(&T::WIRE_ID.to_le_bytes());
    out.push(0);
    let mut parts = Parts {
        out,
        bits: 0,
        name: type_name::<T>(),
    };
    let written = write(&mut parts);
    let bits = parts.bits;
    match written {
        Ok(()) if bits != 0 => {
            out[start + 2] = bits;
            Ok(true)
        }
        Ok(()) => {
            out.truncate(start);
            Ok(false)
        }
        Err(error) => {
            out.truncate(start);
            Err(error)
        }
    }
}

/// The parts of a section being written.
pub(crate) struct Parts<'a> {
    out: &'a mut Vec<u8>,
    bits: u8,
    name: &'static str,
}

impl Parts<'_> {
    /// Appends part `bit` with the items `items` writes, or nothing when it
    /// writes none.
    pub(crate) fn part(
        &mut self,
        bit: u8,
        items: impl FnOnce(&mut Vec<u8>) -> Result<(), EncodeError>,
    ) -> Result<(), EncodeError> {
        let part = self.out.len();
        self.out.extend_from_slice(&[0; 4]);
        items(self.out)?;
        let length = self.out.len() - part - 4;
        if length == 0 {
            self.out.truncate(part);
            return Ok(());
        }
        let length = u32::try_from(length)
            .map_err(|_| EncodeError::new(self.name, EncodeErrorKind::TooLarge))?;
        self.out[part..part + 4].copy_from_slice(&length.to_le_bytes());
        self.bits |= bit;
        Ok(())
    }

    /// Appends the removed part.
    pub(crate) fn removed(&mut self, removed: &[Uid]) -> Result<(), EncodeError> {
        self.part(REMOVED, |out| {
            for &uid in removed {
                put_uid(out, uid);
            }
            Ok(())
        })
    }
}

pub(crate) fn put_uid(out: &mut Vec<u8>, uid: Uid) {
    out.extend_from_slice(&uid.to_bits().to_le_bytes());
}

/// Appends `uid | value`, the value behind a length prefix unless every
/// encoding has the same size.
pub(crate) fn put_value<T: Valued>(
    out: &mut Vec<u8>,
    uid: Uid,
    value: &T,
) -> Result<(), EncodeError> {
    match T::SIZE {
        Some(size) => {
            put_uid(out, uid);
            put_bytes(out, size, |buf| value.encode(buf))
        }
        None => put_sized(
            out,
            uid,
            prefix_width(T::MAX_ENCODED_SIZE),
            value.encoded_size(),
            |buf| value.encode(buf),
        ),
    }
}

/// Appends `uid | len | bytes`, where `encode` writes the `size` bytes and
/// `len` is `width` bytes wide.
pub(crate) fn put_sized(
    out: &mut Vec<u8>,
    uid: Uid,
    width: usize,
    size: usize,
    encode: impl FnOnce(&mut [u8]) -> Result<usize, EncodeError>,
) -> Result<(), EncodeError> {
    put_uid(out, uid);
    debug_assert!(size < 1 << (8 * width));
    out.extend_from_slice(&size.to_le_bytes()[..width]);
    put_bytes(out, size, encode)
}

fn put_bytes(
    out: &mut Vec<u8>,
    size: usize,
    encode: impl FnOnce(&mut [u8]) -> Result<usize, EncodeError>,
) -> Result<(), EncodeError> {
    let at = out.len();
    out.resize(at + size, 0);
    encode(&mut out[at..])?;
    Ok(())
}

/// The width of a length prefix before items whose largest encoding is
/// `max`: 1 byte when it fits, 2 otherwise.
pub(crate) fn prefix_width(max: Option<usize>) -> usize {
    match max {
        Some(max) if max <= u8::MAX as usize => 1,
        _ => 2,
    }
}

/// Reads the section at the start of `buf`, which begins after the
/// component id, and returns its length. Calls `part` with each part's bit,
/// items, and where they start. A part outside `allowed` is an error.
pub(crate) fn decode<T>(
    buf: &[u8],
    allowed: u8,
    mut part: impl FnMut(u8, &[u8], usize) -> Result<(), DecodeError>,
) -> Result<usize, DecodeError> {
    let bits = *buf
        .first()
        .ok_or_else(|| error::<T>(0, DecodeErrorKind::Size))?;
    if bits & !allowed != 0 {
        return Err(error::<T>(0, DecodeErrorKind::Padding));
    }
    let mut at = 1;
    for bit in [VALUES, CHANGES, REMOVED] {
        if bits & bit != 0 {
            let length = buf
                .get(at..at + 4)
                .ok_or_else(|| error::<T>(at, DecodeErrorKind::Size))?;
            let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
            let start = at + 4;
            let items = buf
                .get(start..start + length)
                .ok_or_else(|| error::<T>(start, DecodeErrorKind::Size))?;
            at = start + length;
            part(bit, items, start)?;
        }
    }
    Ok(at)
}

/// Reads a part of `uid` items, which start at `at` of the section.
pub(crate) fn read_uids<T>(
    items: &[u8],
    at: usize,
    mut item: impl FnMut(Uid),
) -> Result<(), DecodeError> {
    if !items.len().is_multiple_of(8) {
        return Err(error::<T>(at, DecodeErrorKind::Length));
    }
    for pos in (0..items.len()).step_by(8) {
        item(read_uid::<T>(items, pos, at)?);
    }
    Ok(())
}

/// Reads a part of `uid | [len] | bytes` items, which start at `at` of the
/// section: each `size` bytes, or behind a `len` of `width` bytes when
/// `size` is `None`.
pub(crate) fn read_sized<T>(
    items: &[u8],
    at: usize,
    size: Option<usize>,
    width: usize,
    mut item: impl FnMut(Uid, &[u8]) -> Result<(), DecodeError>,
) -> Result<(), DecodeError> {
    let mut pos = 0;
    while pos < items.len() {
        let uid = read_uid::<T>(items, pos, at)?;
        pos += 8;
        let size = match size {
            Some(size) => size,
            None => {
                let prefix = items
                    .get(pos..pos + width)
                    .ok_or_else(|| error::<T>(at + pos, DecodeErrorKind::Size))?;
                pos += width;
                prefix
                    .iter()
                    .rev()
                    .fold(0, |size, &byte| size << 8 | byte as usize)
            }
        };
        let bytes = items
            .get(pos..pos + size)
            .ok_or_else(|| error::<T>(at + pos, DecodeErrorKind::Size))?;
        pos += size;
        item(uid, bytes)?;
    }
    Ok(())
}

/// Reads a part of [`put_value`] items.
pub(crate) fn read_values<T: Valued>(
    items: &[u8],
    at: usize,
    mut item: impl FnMut(Uid, T),
) -> Result<(), DecodeError> {
    let width = prefix_width(T::MAX_ENCODED_SIZE);
    read_sized::<T>(items, at, T::SIZE, width, |uid, bytes| {
        item(uid, T::decode(bytes)?);
        Ok(())
    })
}

fn read_uid<T>(items: &[u8], pos: usize, at: usize) -> Result<Uid, DecodeError> {
    let bytes = items
        .get(pos..pos + 8)
        .ok_or_else(|| error::<T>(at + pos, DecodeErrorKind::Size))?;
    let bits = u64::from_le_bytes(bytes.try_into().unwrap());
    if bits >> 32 == 0 {
        return Err(error::<T>(at + pos, DecodeErrorKind::Range));
    }
    Ok(Uid::from_bits(bits))
}

fn error<T>(offset: usize, kind: DecodeErrorKind) -> DecodeError {
    DecodeError::new(offset, type_name::<T>(), kind)
}

/// Moving a table's state between two worlds, as the runtime does.
#[cfg(test)]
pub(crate) mod testing {
    use std::num::NonZeroU32;

    use crate::ecs::component::Component;
    use crate::ecs::entity::{Entity, Uid, UidAllocator};
    use crate::ecs::query::Query;
    use crate::ecs::world::World;

    pub(crate) fn uid(serial: u32) -> Uid {
        Uid::new(NonZeroU32::new(1).unwrap(), serial)
    }

    /// A world that allocates uids, as a sim does.
    pub(crate) fn sim() -> World {
        let world = World::new();
        world.install_uid_allocator(UidAllocator::new(NonZeroU32::MIN, NonZeroU32::MAX));
        world
    }

    /// Encodes `C`'s changes on `sim` as one section, as the encoder does:
    /// through the untyped table, with uids from the registry. `full` lists
    /// the entities sent whole, such as those that entered.
    pub(crate) fn encode_changes<C: Component>(sim: &World, full: &[Entity]) -> Vec<u8> {
        let table = sim.tables[C::id() as usize].borrow();
        let named = |entities: &mut dyn Iterator<Item = Entity>| {
            entities
                .filter_map(|e| Some((sim.uid(e)?, e)))
                .collect::<Vec<_>>()
        };
        let mut changed = Vec::new();
        table.changed(&mut changed);
        changed.retain(|e| !full.contains(e));
        let removed = table
            .removed()
            .iter()
            .filter(|&&e| sim.is_alive(e) && !table.contains(e))
            .filter_map(|&e| sim.uid(e))
            .collect::<Vec<_>>();

        let mut out = Vec::new();
        let full = named(&mut full.iter().copied());
        let changed = named(&mut changed.into_iter());
        table
            .encode_section(&full, &changed, &removed, &mut out)
            .unwrap();
        out
    }

    /// Applies one section on `edge`, adopting the uids it does not hold
    /// as replicas. Returns the uids whose ops failed to apply.
    pub(crate) fn apply<C: Component>(edge: &World, section: &[u8]) -> Vec<Uid> {
        let id = u16::from_le_bytes([section[0], section[1]]);
        let mut table = edge.tables[C::id() as usize].borrow_mut();
        assert_eq!(table.replication().unwrap().wire_id, id);
        let mut resolve = |uid| {
            let entity = edge.resolve(uid).or_else(|| edge.adopt(uid))?;
            edge.entities().set_replica(entity, true);
            Some(entity)
        };
        let mut failed = Vec::new();
        let read = table
            .apply_section(&section[2..], &mut resolve, &mut failed)
            .unwrap();
        assert_eq!(read, section.len() - 2);
        failed
    }

    /// `C`'s values on `world` by uid, in uid order.
    pub(crate) fn values<C: Component + Clone>(world: &World) -> Vec<(Uid, C)> {
        let mut query = Query::<C>::new(world);
        let mut values = query
            .iter()
            .map(|(e, value)| (world.uid(e).unwrap(), value.clone()))
            .collect::<Vec<_>>();
        values.sort_by_key(|(uid, _)| *uid);
        values
    }
}
