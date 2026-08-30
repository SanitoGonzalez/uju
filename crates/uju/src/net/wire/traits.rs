//! The wire traits and the layout arithmetic shared by owned values, views,
//! and generated code; the format itself is specified in `docs/schema/wire.md`.
//!
//! Offsets are absolute in the message buffer, because padding is measured
//! from the start of the message: a value never moves relative to its
//! alignment, so a payload's size is the same wherever a well-aligned encoder
//! puts it, and [`Wire::encoded_size`] can be computed before writing.

use core::cmp::Ordering;

use crate::net::wire::error::{Result, need};
use crate::net::wire::read::read_u16;
use crate::net::wire::write::Writer;

/// Size and alignment of a wire type, shared by an owned value and its view.
pub trait Layout {
    /// `Some(size)` for a fixed-size type, `None` for a variable-size one.
    const FIXED_SIZE: Option<usize>;

    /// Alignment of the value, or of a variable-size value's payload.
    const ALIGN: usize;
}

/// An owned value that can be encoded.
pub trait Wire: Layout {
    /// Bytes the value occupies: the whole value when fixed-size, the payload
    /// alone — without the `u16` head — when variable-size.
    fn encoded_size(&self) -> usize;

    /// Writes the value at the writer's position, which the caller has
    /// aligned to [`Layout::ALIGN`].
    fn encode(&self, w: &mut Writer);

    /// The `u16` preceding a variable-size payload: an item count, a byte
    /// length, or a union tag. Fixed-size values have no head.
    fn head(&self) -> usize {
        0
    }
}

/// A zero-copy view of an encoded value, borrowed from the message buffer.
///
/// [`View::read`] and [`View::end`] assume [`View::validate`] has accepted the
/// bytes; on anything else they read nonsense or panic.
pub trait View<'a>: Layout + Copy + Sized {
    type Owned;

    /// Reads a fixed-size value at `at`, or the payload of a variable-size
    /// value whose `head` has already been read.
    fn read(bytes: &'a [u8], at: usize, head: u16) -> Self;

    /// Offset just past the value.
    fn end(bytes: &'a [u8], at: usize, head: u16) -> usize;

    /// Checks the value and returns the offset just past it.
    fn validate(bytes: &'a [u8], at: usize, head: u16) -> Result<usize>;

    /// Copies the value out of the buffer.
    fn to_owned(self) -> Self::Owned;
}

/// The total order `set` items and `map` keys are sorted by on the wire:
/// numeric for integers, [`f64::total_cmp`] for floats, field-wise in
/// declaration order for structs.
pub trait Canonical {
    fn canonical_cmp(&self, other: &Self) -> Ordering;
}

/// Rounds `at` up to a multiple of `align`.
pub const fn pad(at: usize, align: usize) -> usize {
    at.next_multiple_of(align)
}

/// Size of a fixed-size type, or zero for a variable-size one.
pub const fn fixed_size(fixed: Option<usize>) -> usize {
    match fixed {
        Some(size) => size,
        None => 0,
    }
}

/// Alignment of the `[u16 head][padding][payload]` block a variable-size
/// value takes inside a container: enough for the head, and for the payload.
pub const fn block_align(align: usize) -> usize {
    if align > 2 { align } else { 2 }
}

/// Alignment of one container element: a fixed-size value is packed at its own
/// alignment, a variable-size one takes a block.
pub const fn element_align(fixed: Option<usize>, align: usize) -> usize {
    match fixed {
        Some(_) => align,
        None => block_align(align),
    }
}

/// Layout of one `map` entry whose value is fixed-size: the key and the value
/// packed like struct fields — higher alignment first, the key winning ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub size: usize,
    pub align: usize,
    /// Offset of the key from the start of the entry.
    pub key: usize,
    /// Offset of the value from the start of the entry.
    pub value: usize,
}

pub const fn entry(
    key_size: usize,
    key_align: usize,
    value_size: usize,
    value_align: usize,
) -> Entry {
    let align = if key_align > value_align {
        key_align
    } else {
        value_align
    };
    let (key, value, end) = if key_align >= value_align {
        let value = pad(key_size, value_align);
        (0, value, value + value_size)
    } else {
        let key = pad(value_size, key_align);
        (key, 0, key + key_size)
    };
    Entry {
        size: pad(end, align),
        align,
        key,
        value,
    }
}

/// Reads a fixed-size value at `at`.
pub fn read_fixed<'a, T: View<'a>>(bytes: &'a [u8], at: usize) -> T {
    T::read(bytes, at, 0)
}

/// Checks a fixed-size value at `at`.
pub fn validate_fixed<'a, T: View<'a>>(bytes: &'a [u8], at: usize) -> Result<usize> {
    T::validate(bytes, at, 0)
}

/// Reads a value whose `head` was carried elsewhere — in a message slot, or in
/// the head of the enclosing block.
pub fn read_value<'a, T: View<'a>>(bytes: &'a [u8], at: usize, head: u16) -> T {
    T::read(bytes, at, head)
}

/// Offset just past a value whose `head` was carried elsewhere.
pub fn value_end<'a, T: View<'a>>(bytes: &'a [u8], at: usize, head: u16) -> usize {
    T::end(bytes, at, head)
}

/// Checks a value whose `head` was carried elsewhere.
pub fn validate_value<'a, T: View<'a>>(bytes: &'a [u8], at: usize, head: u16) -> Result<usize> {
    T::validate(bytes, at, head)
}

/// Offset of the payload inside a container element: a fixed-size value starts
/// at `at`, a variable-size one follows its head.
fn payload_at<'a, T: View<'a>>(at: usize) -> usize {
    match T::FIXED_SIZE {
        Some(_) => at,
        None => pad(at + 2, T::ALIGN),
    }
}

/// Reads one container element at `at`: a fixed-size value, or a block.
pub fn read_element<'a, T: View<'a>>(bytes: &'a [u8], at: usize) -> T {
    T::read(bytes, payload_at::<T>(at), element_head::<T>(bytes, at))
}

/// Offset just past one container element at `at`.
pub fn element_end<'a, T: View<'a>>(bytes: &'a [u8], at: usize) -> usize {
    match T::FIXED_SIZE {
        Some(size) => at + size,
        None => T::end(bytes, payload_at::<T>(at), element_head::<T>(bytes, at)),
    }
}

/// Checks one container element at `at`.
pub fn validate_element<'a, T: View<'a>>(bytes: &'a [u8], at: usize) -> Result<usize> {
    if T::FIXED_SIZE.is_none() {
        need(bytes, at + 2)?;
    }
    T::validate(bytes, payload_at::<T>(at), element_head::<T>(bytes, at))
}

fn element_head<'a, T: View<'a>>(bytes: &'a [u8], at: usize) -> u16 {
    match T::FIXED_SIZE {
        Some(_) => 0,
        None => read_u16(bytes, at),
    }
}

/// Bytes one container element occupies, from an [`element_align`]-aligned
/// start: a fixed-size value, or a `[u16 head][padding][payload]` block.
pub fn element_size<T: Wire>(value: &T) -> usize {
    match T::FIXED_SIZE {
        Some(size) => size,
        None => pad(2, T::ALIGN) + value.encoded_size(),
    }
}

/// Writes one container element at the writer's position, which the caller has
/// aligned to [`element_align`].
pub fn encode_element<T: Wire>(value: &T, w: &mut Writer) {
    if T::FIXED_SIZE.is_none() {
        w.push_head(value.head());
        w.align(T::ALIGN);
    }
    value.encode(w);
}

/// Encoded size of a container payload holding `len` items.
pub fn items_size<'i, T: Wire + 'i>(items: impl Iterator<Item = &'i T>, len: usize) -> usize {
    match T::FIXED_SIZE {
        Some(size) => len * size,
        None => items.fold(0, |size, item| {
            pad(size, block_align(T::ALIGN)) + element_size(item)
        }),
    }
}

/// Writes a container payload: fixed-size items packed at their stride,
/// variable-size ones as consecutive blocks.
pub fn encode_items<'i, T: Wire + 'i>(items: impl Iterator<Item = &'i T>, w: &mut Writer) {
    let align = element_align(T::FIXED_SIZE, T::ALIGN);
    for item in items {
        w.align(align);
        encode_element(item, w);
    }
}
