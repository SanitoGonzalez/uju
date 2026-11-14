//! Sizes and layouts: how wide each integer, count, length and offset is,
//! and where each field of an object or variant sits.

use super::*;
use crate::ir::{DefKind, Field, Number, Replicate, TypeKind, VariantData};

/// Byte width of an integer on the wire, little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Width {
    U8,
    U16,
    U32,
    U64,
}

impl Width {
    pub fn bytes(self) -> u32 {
        match self {
            Width::U8 => 1,
            Width::U16 => 2,
            Width::U32 => 4,
            Width::U64 => 8,
        }
    }

    /// The narrowest width holding every value up to `max`.
    fn of(max: u128) -> Width {
        if max <= u8::MAX as u128 {
            Width::U8
        } else if max <= u16::MAX as u128 {
            Width::U16
        } else if max <= u32::MAX as u128 {
            Width::U32
        } else {
            Width::U64
        }
    }

    /// The width of a length, count, or offset of at most `max`; without a
    /// bound, the message size bounds it.
    fn length(max: Option<u32>) -> Width {
        Width::of(max.unwrap_or(MAX_SIZE) as u128)
    }
}

/// How many bytes a value takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// Always exactly this many.
    Fixed(u32),
    /// It varies, up to `max`; `None` when only [`MAX_SIZE`] bounds it.
    Var { max: Option<u32> },
}

impl Size {
    pub fn fixed(self) -> Option<u32> {
        match self {
            Size::Fixed(size) => Some(size),
            Size::Var { .. } => None,
        }
    }

    /// The most bytes a value can take, `None` when unbounded.
    pub fn max(self) -> Option<u32> {
        match self {
            Size::Fixed(size) => Some(size),
            Size::Var { max } => max,
        }
    }
}

/// How an integer is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Int {
    /// As declared: two's complement for signed types.
    Native(Width),
    /// `value - min`, unsigned. Chosen when `min`/`max` fit a narrower width
    /// than the declared type: `i32[min=-100, max=100]` takes one byte.
    Biased { min: i128, width: Width },
}

impl Int {
    /// How a value of `ty` is stored, when `ty` is an integer.
    pub fn of(ty: &Type) -> Option<Int> {
        let (declared, lowest, highest) = match ty.kind {
            TypeKind::I8 => (Width::U8, i8::MIN as i128, i8::MAX as i128),
            TypeKind::I16 => (Width::U16, i16::MIN as i128, i16::MAX as i128),
            TypeKind::I32 => (Width::U32, i32::MIN as i128, i32::MAX as i128),
            TypeKind::I64 => (Width::U64, i64::MIN as i128, i64::MAX as i128),
            TypeKind::U8 => (Width::U8, 0, u8::MAX as i128),
            TypeKind::U16 => (Width::U16, 0, u16::MAX as i128),
            TypeKind::U32 => (Width::U32, 0, u32::MAX as i128),
            TypeKind::U64 => (Width::U64, 0, u64::MAX as i128),
            _ => return None,
        };
        let bound = |bound: Option<Number>, default: i128| match bound {
            Some(Number::Int(value)) => value,
            _ => default,
        };
        let min = bound(ty.constraints.min, lowest);
        let max = bound(ty.constraints.max, highest);
        let width = Width::of((max - min) as u128);
        Some(if width < declared {
            Int::Biased { min, width }
        } else {
            Int::Native(declared)
        })
    }

    pub fn width(self) -> Width {
        match self {
            Int::Native(width) | Int::Biased { width, .. } => width,
        }
    }
}

/// How a `vec`, `set`, or `map` lays out its items. A map's items are its
/// entries: the key, then the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sequence {
    /// Items of `item` bytes back to back; their count is the length divided
    /// by `item`.
    Packed { item: u32 },
    /// Items of no size, so only their count is sent.
    Counted { count: Width },
    /// Items of varying size: a count, then each item behind its length but
    /// the last, which runs to the end.
    Prefixed { count: Width, prefix: Width },
}

/// Layout of a struct, message, hub kind, or component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub size: Size,
    /// Bytes of the bitmap opening the head.
    pub bitmap: u32,
    /// Offset of the first boundary, right after the fixed fields.
    pub boundaries: u32,
    /// Width of each boundary.
    pub boundary: Width,
    /// Bytes of the head: the bitmap, the fixed fields, and `vars - 1`
    /// boundaries.
    pub head: u32,
    /// Number of slots in the var section.
    pub vars: u32,
    /// Where each field sits, in declaration order.
    pub fields: Vec<Slot>,
}

/// Layout of a union: its tag, then the variant's data, which runs to the
/// end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionLayout {
    /// Always [`Size::Var`].
    pub size: Size,
    /// Width of the tag opening the union: variant `i` is tagged `i + 1`.
    pub tag: Width,
    /// The layout of each variant with fields, indexed by variant; `None`
    /// for the others.
    pub variants: Vec<Option<Layout>>,
}

impl UnionLayout {
    /// The tag of the variant at `index`; `0` is never a tag.
    pub fn tag_of(index: usize) -> u16 {
        u16::try_from(index + 1).expect("lowering bounds the variants")
    }
}

/// Where a field sits. Bits count from the least significant bit of the
/// bitmap's first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// A `bool`, stored as this bit.
    Bit(u32),
    /// A fixed-size value, this many bytes into the head.
    At(u32),
    /// An `option<bool>`: whether it is present, then its value.
    OptionalBit { presence: u32, value: u32 },
    /// An optional fixed-size value. Present ones are packed back to back
    /// after the head, in declaration order.
    Optional { presence: u32 },
    /// A variable-size value in this slot of the var section. An optional
    /// one also has a presence bit, and its slot is empty when absent.
    Var { slot: u32, presence: Option<u32> },
}

/// Layout of a `@replicate(patch)` component's patch: an object whose every
/// field is optional, opened by one dirty bit per field so the sim's dirty
/// mask is copied in as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLayout {
    /// Always [`Size::Var`].
    pub size: Size,
    /// Bytes of the bitmap opening the head: the dirty bits, then the extra
    /// bits.
    pub bitmap: u32,
    /// Offset of the first boundary, right after the bitmap.
    pub boundaries: u32,
    /// Width of each boundary.
    pub boundary: Width,
    /// Bytes of the head: the bitmap and `vars - 1` boundaries.
    pub head: u32,
    /// Number of slots in the var section.
    pub vars: u32,
    /// Where each field sits, in declaration order.
    pub fields: Vec<PatchSlot>,
}

/// Where a field sits in a patch. Bits count from the least significant bit
/// of the bitmap's first byte, and are zero when unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchSlot {
    /// Set when the field is sent: its declaration index.
    pub dirty: u32,
    /// For an `option` field, set when it is sent as `Some`.
    pub some: Option<u32>,
    /// Where its value sits when sent.
    pub value: PatchValue,
}

/// Where a sent field's value sits in a patch. A clean field, or one sent
/// as `None`, has no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchValue {
    /// A `bool`, stored as this bit.
    Bit(u32),
    /// A fixed-size value. Values are packed back to back after the head,
    /// in declaration order.
    Packed,
    /// A variable-size value in this slot of the var section, which is
    /// empty when there is no value.
    Var(u32),
}

/// The size of every declaration, and the layout of each object, union and
/// patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Layouts {
    /// Indexed by [`TypeId`].
    pub(super) sizes: Vec<Size>,
    /// Indexed by [`TypeId`]; `Some` for structs, messages, hub kinds, and
    /// components.
    pub(super) layouts: Vec<Option<Layout>>,
    /// Indexed by [`TypeId`]; `Some` for unions.
    pub(super) unions: Vec<Option<UnionLayout>>,
    /// Indexed by [`TypeId`]; `Some` for `@replicate(patch)` components.
    pub(super) patches: Vec<Option<PatchLayout>>,
}

impl Layouts {
    pub(super) fn new(schema: &Schema, errors: &mut Vec<Error>) -> Layouts {
        let len = schema.defs.len();
        let mut builder = Builder {
            schema,
            states: vec![State::Todo; len],
            layouts: vec![None; len],
            unions: vec![None; len],
            patches: vec![None; len],
            errors: Vec::new(),
        };
        let sizes = (0..len).map(|index| builder.def(TypeId(index))).collect();
        errors.append(&mut builder.errors);
        Layouts {
            sizes,
            layouts: builder.layouts,
            unions: builder.unions,
            patches: builder.patches,
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Todo,
    /// Being laid out. Never reached again, as lowering rejects recursive
    /// types.
    Busy,
    Done(Size),
}

struct Builder<'a> {
    schema: &'a Schema,
    states: Vec<State>,
    layouts: Vec<Option<Layout>>,
    unions: Vec<Option<UnionLayout>>,
    patches: Vec<Option<PatchLayout>>,
    errors: Vec<Error>,
}

impl Builder<'_> {
    fn def(&mut self, id: TypeId) -> Size {
        match self.states[id.0] {
            State::Done(size) => return size,
            State::Busy => unreachable!("lowering rejects recursive types"),
            State::Todo => {}
        }
        self.states[id.0] = State::Busy;
        let schema = self.schema;
        let size = match &schema.def(id).kind {
            DefKind::Enum(item) => Size::Fixed(item.repr.size() as u32),
            DefKind::Struct(item) => self.object(id, &item.fields),
            DefKind::Message(item) => self.object(id, &item.fields),
            DefKind::Hub(item) => {
                // The key travels in envelopes, so it must fit as a value.
                if let Some(size) = self.oversized(&item.key) {
                    self.errors.push(Error {
                        def: id,
                        variant: None,
                        field: None,
                        message: format!(
                            "`{}` takes {size} bytes; a message holds at most {MAX_SIZE}",
                            self.schema.type_name(&item.key)
                        ),
                    });
                }
                self.object(id, &item.fields)
            }
            DefKind::Component(item) => {
                let errors = self.errors.len();
                let size = self.object(id, &item.fields);
                if item.replicate == Replicate::Patch {
                    let reported = self.errors[errors..].iter().any(|error| error.def == id);
                    self.patches[id.0] = Some(self.patch(id, &item.fields, reported));
                }
                size
            }
            DefKind::Audience(item) => {
                // The key travels in frames, so it must fit as a value.
                if let Some(size) = self.oversized(&item.key) {
                    self.errors.push(Error {
                        def: id,
                        variant: None,
                        field: None,
                        message: format!(
                            "`{}` takes {size} bytes; a message holds at most {MAX_SIZE}",
                            self.schema.type_name(&item.key)
                        ),
                    });
                }
                self.size(&item.key)
            }
            // No value: only its addition and removal travel.
            DefKind::Tag(_) => Size::Fixed(0),
            DefKind::Union(item) => self.union(id, &item.variants),
            // A scalar, so never too large.
            DefKind::Type(item) => self.size(&item.target),
        };
        self.states[id.0] = State::Done(size);
        size
    }

    fn size(&mut self, ty: &Type) -> Size {
        type_size(ty, &mut |id| self.def(id))
    }

    fn object(&mut self, id: TypeId, fields: &[Field]) -> Size {
        let layout = self.layout(id, None, fields);
        let size = layout.size;
        self.layouts[id.0] = Some(layout);
        size
    }

    fn union(&mut self, id: TypeId, variants: &[crate::ir::UnionVariant]) -> Size {
        let tag = Width::of(variants.len() as u128);
        let mut layouts = Vec::with_capacity(variants.len());
        // The largest variant's data, `None` once one is unbounded.
        let mut largest = Some(0u32);
        for (index, variant) in variants.iter().enumerate() {
            let (size, layout) = match &variant.data {
                VariantData::Unit => (Size::Fixed(0), None),
                VariantData::Type(ty) => {
                    if let Some(size) = self.oversized(ty) {
                        self.errors.push(Error {
                            def: id,
                            variant: Some(index),
                            field: None,
                            message: format!(
                                "`{}` takes {size} bytes; a message holds at most {MAX_SIZE}",
                                self.schema.type_name(ty)
                            ),
                        });
                    }
                    (self.size(ty), None)
                }
                VariantData::Fields(fields) => {
                    let layout = self.layout(id, Some(index), fields);
                    (layout.size, Some(layout))
                }
            };
            largest = largest.zip(size.max()).map(|(a, b)| a.max(b));
            layouts.push(layout);
        }
        let size = Size::Var {
            max: largest.and_then(|largest| bound(tag.bytes() as u64 + largest as u64)),
        };
        self.unions[id.0] = Some(UnionLayout {
            size,
            tag,
            variants: layouts,
        });
        size
    }

    /// Lays out the fields of a struct, message, hub kind, or component, or
    /// of a union's
    /// variant.
    fn layout(&mut self, id: TypeId, variant: Option<usize>, fields: &[Field]) -> Layout {
        let mut bits = 0;
        let mut bit = || {
            bits += 1;
            bits - 1
        };
        let mut slots = Vec::with_capacity(fields.len());
        let mut sizes = Vec::with_capacity(fields.len());
        let mut vars = 0;
        let mut oversized = false;
        for (index, field) in fields.iter().enumerate() {
            let size = self.size(&field.ty);
            if let Some(size) = self.oversized(&field.ty) {
                oversized = true;
                self.errors.push(Error {
                    def: id,
                    variant,
                    field: Some(index),
                    message: format!(
                        "`{}` takes {size} bytes; a message holds at most {MAX_SIZE}",
                        self.schema.type_name(&field.ty)
                    ),
                });
            }
            // A `type` over `bool` is a bit too.
            let kind = &self.schema.resolved(&field.ty).kind;
            let slot = match (field.optional, kind, size) {
                (false, TypeKind::Bool, _) => Slot::Bit(bit()),
                (true, TypeKind::Bool, _) => Slot::OptionalBit {
                    presence: bit(),
                    value: bit(),
                },
                // Offsets are filled in once the bitmap's size is known.
                (false, _, Size::Fixed(_)) => Slot::At(0),
                (true, _, Size::Fixed(_)) => Slot::Optional { presence: bit() },
                (optional, _, Size::Var { .. }) => {
                    vars += 1;
                    Slot::Var {
                        slot: vars - 1,
                        presence: optional.then(&mut bit),
                    }
                }
            };
            slots.push(slot);
            sizes.push(size);
        }

        let bitmap = bits.div_ceil(8);
        let mut offset = bitmap as u64;
        let mut optional = 0u64;
        let mut var: Option<u64> = Some(0);
        for (slot, size) in slots.iter_mut().zip(&sizes) {
            match slot {
                Slot::At(at) => {
                    *at = offset as u32;
                    offset += size.max().unwrap_or(0) as u64;
                }
                Slot::Optional { .. } => optional += size.max().unwrap_or(0) as u64,
                Slot::Var { .. } => {
                    var = var.zip(size.max()).map(|(sum, max)| sum + max as u64);
                }
                Slot::Bit(_) | Slot::OptionalBit { .. } => {}
            }
        }
        let boundaries = offset;
        let (boundary, head) = head(boundaries, vars, var.map(|var| optional + var));

        if head > MAX_SIZE as u64 && !oversized {
            let mut name = self.schema.qualified(id);
            if let (Some(index), DefKind::Union(item)) = (variant, &self.schema.def(id).kind) {
                name.push('.');
                name.push_str(&item.variants[index].name);
            }
            self.errors.push(Error {
                def: id,
                variant,
                field: None,
                message: format!(
                    "`{name}` takes at least {head} bytes; a message holds at most {MAX_SIZE}"
                ),
            });
        }
        // An `option<bool>` lives in the bitmap, so only these vary in size.
        let varies = |slot: &Slot| matches!(slot, Slot::Optional { .. } | Slot::Var { .. });
        let size = if !slots.iter().any(varies) {
            Size::Fixed(head.min(u32::MAX as u64) as u32)
        } else {
            Size::Var {
                max: var.and_then(|var| bound(head + optional + var)),
            }
        };
        Layout {
            size,
            bitmap,
            boundaries: boundaries.min(u32::MAX as u64) as u32,
            boundary,
            head: head.min(u32::MAX as u64) as u32,
            vars,
            fields: slots,
        }
    }

    /// Lays out the patch of a `@replicate(patch)` component. `reported`
    /// tells whether the component already reported itself too large, in
    /// which case its patch does not again.
    fn patch(&mut self, id: TypeId, fields: &[Field], reported: bool) -> PatchLayout {
        // The dirty bits come first, one per field.
        let mut bits = fields.len() as u32;
        let mut bit = || {
            bits += 1;
            bits - 1
        };
        let mut slots = Vec::with_capacity(fields.len());
        let mut vars = 0;
        let mut packed = 0u64;
        let mut var: Option<u64> = Some(0);
        for (index, field) in fields.iter().enumerate() {
            let size = self.size(&field.ty);
            let some = field.optional.then(&mut bit);
            // A `type` over `bool` is a bit too.
            let value = match (&self.schema.resolved(&field.ty).kind, size) {
                (TypeKind::Bool, _) => PatchValue::Bit(bit()),
                (_, Size::Fixed(size)) => {
                    packed += size as u64;
                    PatchValue::Packed
                }
                (_, Size::Var { max }) => {
                    var = var.zip(max).map(|(sum, max)| sum + max as u64);
                    vars += 1;
                    PatchValue::Var(vars - 1)
                }
            };
            slots.push(PatchSlot {
                dirty: index as u32,
                some,
                value,
            });
        }

        let bitmap = bits.div_ceil(8);
        let (boundary, head) = head(bitmap as u64, vars, var.map(|var| packed + var));
        if head > MAX_SIZE as u64 && !reported {
            self.errors.push(Error {
                def: id,
                variant: None,
                field: None,
                message: format!(
                    "the patch of `{}` takes at least {head} bytes; a message holds at most {MAX_SIZE}",
                    self.schema.qualified(id)
                ),
            });
        }
        PatchLayout {
            size: Size::Var {
                max: var.and_then(|var| bound(head + packed + var)),
            },
            bitmap,
            boundaries: bitmap,
            boundary,
            head: head.min(u32::MAX as u64) as u32,
            vars,
            fields: slots,
        }
    }

    /// The size of a fixed-size part of `ty` too large for any message.
    /// Named types are skipped, as they report themselves.
    fn oversized(&mut self, ty: &Type) -> Option<u32> {
        match &ty.kind {
            TypeKind::Named(_) => None,
            TypeKind::Vec(item) | TypeKind::Set(item) => self.oversized(item),
            TypeKind::Map(key, value) => self.oversized(key).or_else(|| self.oversized(value)),
            _ => self.size(ty).fixed().filter(|size| *size > MAX_SIZE),
        }
    }
}

/// The width of an object's `vars - 1` boundaries, which start at
/// `boundaries`, and the size of the head they end. `rest` is the most the
/// optional and var sections take, `None` when unbounded. Boundaries are
/// narrow when the whole object, laid out with them, fits.
fn head(boundaries: u64, vars: u32, rest: Option<u64>) -> (Width, u64) {
    let count = vars.saturating_sub(1) as u64;
    let narrow = rest.map(|rest| boundaries + count + rest);
    let boundary = Width::length(narrow.and_then(bound));
    (boundary, boundaries + count * boundary.bytes() as u64)
}

/// `size` as a bound on a value inside a message: `None` when the message
/// size bounds it more tightly.
fn bound(size: u64) -> Option<u32> {
    (size <= MAX_SIZE as u64).then_some(size as u32)
}

pub(super) fn type_size(ty: &Type, def: &mut impl FnMut(TypeId) -> Size) -> Size {
    let fixed = match &ty.kind {
        TypeKind::I8
        | TypeKind::I16
        | TypeKind::I32
        | TypeKind::I64
        | TypeKind::U8
        | TypeKind::U16
        | TypeKind::U32
        | TypeKind::U64 => Int::of(ty).expect("an integer").width().bytes(),
        // A `bool` outside a bitmap, as in a `vec<bool>`, takes a byte.
        TypeKind::Bool => 1,
        TypeKind::F32 => 4,
        TypeKind::F64 | TypeKind::Timestamp | TypeKind::Duration | TypeKind::Entity => 8,
        TypeKind::UEntity => 12,
        TypeKind::Uuid => 16,
        TypeKind::String | TypeKind::Bytes => match ty.constraints.fixed_length() {
            Some(length) => length.min(u32::MAX as u64) as u32,
            None => {
                return Size::Var {
                    max: ty.constraints.max_length.and_then(bound),
                };
            }
        },
        // Lowering keeps array items fixed-size.
        TypeKind::Array(item, len) => match type_size(item, def) {
            Size::Fixed(size) => size.saturating_mul(*len),
            Size::Var { max } => {
                return Size::Var {
                    max: max.and_then(|max| bound(max as u64 * *len as u64)),
                };
            }
        },
        TypeKind::Vec(_) | TypeKind::Set(_) | TypeKind::Map(..) => {
            let (sequence, item) = sequence(ty, def).expect("a sequence");
            return Size::Var {
                max: sequence_max(ty, sequence, item),
            };
        }
        TypeKind::Named(id) => return def(*id),
    };
    Size::Fixed(fixed)
}

/// How `ty` lays out its items, with the size of one item.
pub(super) fn sequence(
    ty: &Type,
    def: &mut impl FnMut(TypeId) -> Size,
) -> Option<(Sequence, Size)> {
    let item = match &ty.kind {
        TypeKind::Vec(item) | TypeKind::Set(item) => type_size(item, def),
        TypeKind::Map(key, value) => match (type_size(key, def), type_size(value, def)) {
            (Size::Fixed(key), Size::Fixed(value)) => Size::Fixed(key.saturating_add(value)),
            (key, value) => Size::Var {
                max: key
                    .max()
                    .zip(value.max())
                    .and_then(|(key, value)| bound(key as u64 + value as u64)),
            },
        },
        _ => return None,
    };
    let count = Width::length(ty.constraints.max_length.and_then(bound));
    let sequence = match item {
        Size::Fixed(0) => Sequence::Counted { count },
        Size::Fixed(item) => Sequence::Packed { item },
        Size::Var { max } => Sequence::Prefixed {
            count,
            prefix: Width::length(max),
        },
    };
    Some((sequence, item))
}

fn sequence_max(ty: &Type, sequence: Sequence, item: Size) -> Option<u32> {
    if let Sequence::Counted { count } = sequence {
        return Some(count.bytes());
    }
    let count = ty.constraints.max_length?;
    let total = match sequence {
        Sequence::Packed { item } => count * item as u64,
        Sequence::Prefixed {
            count: width,
            prefix,
        } => {
            width.bytes() as u64
                + count * item.max()? as u64
                + count.saturating_sub(1) * prefix.bytes() as u64
        }
        Sequence::Counted { .. } => unreachable!("handled above"),
    };
    bound(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;
    use crate::ir::wire::tests::*;

    #[test]
    fn integers_narrow_to_their_range() {
        let cases = [
            (
                "i32[min=-100, max=100]",
                Int::Biased {
                    min: -100,
                    width: Width::U8,
                },
            ),
            (
                "i64[min=0, max=70000]",
                Int::Biased {
                    min: 0,
                    width: Width::U32,
                },
            ),
            (
                "u32[min=1000, max=1255]",
                Int::Biased {
                    min: 1000,
                    width: Width::U8,
                },
            ),
            ("u8[max=9]", Int::Native(Width::U8)),
            ("u32[min=10]", Int::Native(Width::U32)),
            ("i16", Int::Native(Width::U16)),
            ("i8[min=-128, max=127]", Int::Native(Width::U8)),
        ];
        for (ty, expected) in cases {
            let (schema, _, m) = message(&format!("message(client -> sim) M {{ x: {ty} }}"));
            assert_eq!(Int::of(field(&schema, m, "x")), Some(expected), "{ty}");
        }
    }

    #[test]
    fn scalars_have_fixed_sizes() {
        let (schema, wire, m) = message(concat!(
            "enum E: u16 { A }\n",
            "message(client -> sim) M { a: f32, b: timestamp, c: duration, d: entity, e: uentity, ",
            "f: uuid, g: E, h: string[min_length=3, max_length=3], i: array<u16, 3>, ",
            "j: bytes[max_length=4] }",
        ));
        let size = |name| wire.size(field(&schema, m, name));
        let expected = [
            ("a", 4),
            ("b", 8),
            ("c", 8),
            ("d", 8),
            ("e", 12),
            ("f", 16),
            ("g", 2),
            ("h", 3),
            ("i", 6),
        ];
        for (name, bytes) in expected {
            assert_eq!(size(name), Size::Fixed(bytes), "{name}");
        }
        assert_eq!(size("j"), Size::Var { max: Some(4) });
    }

    #[test]
    fn structs_pack_bools_into_a_bitmap() {
        let (schema, wire) = compiled(&[
            "namespace a\nstruct S { a: bool, x: u16, b: bool, y: f64, flags: array<bool, 2> }\n",
        ]);
        let layout = wire.layout(id(&schema, "S"));
        assert_eq!(layout.bitmap, 1);
        assert_eq!(
            layout.fields,
            [
                Slot::Bit(0),
                Slot::At(1),
                Slot::Bit(1),
                Slot::At(3),
                // A bool inside an array takes a byte.
                Slot::At(11),
            ]
        );
        assert_eq!(layout.size, Size::Fixed(13));
        assert_eq!(layout.vars, 0);
    }

    #[test]
    fn empty_declarations_take_no_bytes() {
        let (schema, wire) = compiled(&[EXAMPLE1]);
        assert_eq!(wire.layout(id(&schema, "EmptyStruct")).size, Size::Fixed(0));
        assert_eq!(wire.layout(id(&schema, "Ping")).size, Size::Fixed(0));
    }

    #[test]
    fn sample_request_lays_out_narrow() {
        let (schema, wire) = compiled(&[EXAMPLE1]);
        let request = id(&schema, "MyRequest");
        let layout = wire.layout(request);
        assert_eq!(
            layout.fields,
            [
                Slot::At(0),
                Slot::Var {
                    slot: 0,
                    presence: None
                },
                Slot::Var {
                    slot: 1,
                    presence: None
                },
            ]
        );
        assert_eq!(layout.bitmap, 0);
        assert_eq!(layout.boundaries, 1, "after the one-byte `foo`");
        assert_eq!(
            layout.boundary,
            Width::U8,
            "the whole message fits in 255 bytes"
        );
        assert_eq!(layout.head, 2);
        // 2 (head) + 12 (name) + 1 (count) + 14 * 16 + 13 (prefixes)
        assert_eq!(layout.size, Size::Var { max: Some(252) });

        assert_eq!(
            wire.sequence(field(&schema, request, "addresses")),
            Some(Sequence::Prefixed {
                count: Width::U8,
                prefix: Width::U8,
            })
        );
    }

    #[test]
    fn unbounded_objects_take_wide_boundaries() {
        let (_, wire, m) = message("message(client -> sim) M { a: string, b: string, c: string }");
        let layout = wire.layout(m);
        assert_eq!(layout.boundary, Width::U16);
        assert_eq!(
            layout.head, 4,
            "two boundaries; the last slot runs to the end"
        );
        assert_eq!(layout.size, Size::Var { max: None });
    }

    #[test]
    fn optional_fields_take_presence_bits() {
        let (_, wire, m) = message(concat!(
            "struct P { x: f32 }\n",
            "message(client -> sim) M { flag: bool, o: option<bool>, p: option<P>, s: option<string>, ",
            "n: u8, q: option<u16> }",
        ));
        let layout = wire.layout(m);
        assert_eq!(
            layout.fields,
            [
                Slot::Bit(0),
                Slot::OptionalBit {
                    presence: 1,
                    value: 2
                },
                Slot::Optional { presence: 3 },
                Slot::Var {
                    slot: 0,
                    presence: Some(4)
                },
                Slot::At(1),
                Slot::Optional { presence: 5 },
            ]
        );
        assert_eq!(layout.bitmap, 1);
        assert_eq!(
            layout.head, 2,
            "bitmap and `n`; one var slot needs no boundary"
        );
        assert_eq!(layout.size, Size::Var { max: None });
    }

    #[test]
    fn options_alone_make_an_object_variable_size() {
        let (_, wire, m) = message("struct M { a: option<u32>, b: option<bool> }");
        assert_eq!(wire.layout(m).size, Size::Var { max: Some(5) });
    }

    #[test]
    fn structs_can_hold_variable_size_fields() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "struct Named { id: u32, name: string[max_length=8] }\n",
            "message(sim -> client) M { all: vec<Named>[max_length=4] }\n",
        )]);
        let named = wire.layout(id(&schema, "Named"));
        assert_eq!(
            named.fields,
            [
                Slot::At(0),
                Slot::Var {
                    slot: 0,
                    presence: None
                }
            ]
        );
        assert_eq!(named.size, Size::Var { max: Some(12) });

        let m = id(&schema, "M");
        assert_eq!(
            wire.sequence(field(&schema, m, "all")),
            Some(Sequence::Prefixed {
                count: Width::U8,
                prefix: Width::U8,
            })
        );
        // 1 (count) + 4 * 12 + 3 (prefixes)
        assert_eq!(wire.layout(m).size, Size::Var { max: Some(52) });
    }

    #[test]
    fn sequences_pack_fixed_items() {
        let (schema, wire, m) = message(concat!(
            "struct P { x: f32, y: f32 }\n",
            "struct Empty {}\n",
            "message(client -> sim) M { ps: vec<P>, flags: vec<bool>, empties: vec<Empty>[max_length=9], ",
            "set: set<u16>, fixed: map<u8, P>, var: map<u8, string[max_length=3]> }",
        ));
        let sequence = |name| wire.sequence(field(&schema, m, name)).unwrap();
        assert_eq!(sequence("ps"), Sequence::Packed { item: 8 });
        assert_eq!(sequence("flags"), Sequence::Packed { item: 1 });
        assert_eq!(sequence("empties"), Sequence::Counted { count: Width::U8 });
        assert_eq!(sequence("set"), Sequence::Packed { item: 2 });
        assert_eq!(sequence("fixed"), Sequence::Packed { item: 9 });
        assert_eq!(
            sequence("var"),
            Sequence::Prefixed {
                count: Width::U16,
                prefix: Width::U8,
            }
        );
        assert_eq!(
            wire.size(field(&schema, m, "empties")),
            Size::Var { max: Some(1) }
        );
    }

    #[test]
    fn unions_take_a_tag_then_their_largest_variant() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "union U {\n",
            "    Clear,\n",
            "    Remove(u32),\n",
            "    Add { slot: u16, flag: bool, name: string[max_length=10] },\n",
            "    Hit(bool),\n",
            "}\n",
            "union Units { A, B }\n",
            "union Open { A(u8), B(string) }\n",
            "message(client -> sim) M { u: U, o: option<U>, v: vec<U>[max_length=2] }\n",
        )]);
        let u = wire.union(id(&schema, "U"));
        assert_eq!(u.tag, Width::U8);
        let add = Layout {
            // 1 (tag) + 1 (bitmap) + 2 (slot) + 10 (name)
            size: Size::Var { max: Some(13) },
            bitmap: 1,
            boundaries: 3,
            boundary: Width::U8,
            vars: 1,
            head: 3,
            fields: vec![
                Slot::At(1),
                Slot::Bit(0),
                Slot::Var {
                    slot: 0,
                    presence: None,
                },
            ],
        };
        assert_eq!(u.variants, [None, None, Some(add), None]);
        assert_eq!(u.size, Size::Var { max: Some(14) }, "the tag and `Add`");

        let m = id(&schema, "M");
        assert_eq!(wire.size(field(&schema, m, "u")), u.size);
        assert_eq!(
            wire.layout(m).fields[..2],
            [
                Slot::Var {
                    slot: 0,
                    presence: None
                },
                Slot::Var {
                    slot: 1,
                    presence: Some(0)
                },
            ]
        );
        assert_eq!(
            wire.sequence(field(&schema, m, "v")),
            Some(Sequence::Prefixed {
                count: Width::U8,
                prefix: Width::U8,
            })
        );

        let units = wire.union(id(&schema, "Units"));
        assert_eq!(units.size, Size::Var { max: Some(1) }, "a tag alone");
        assert_eq!(units.variants, [None, None]);
        assert_eq!(
            wire.union(id(&schema, "Open")).size,
            Size::Var { max: None },
            "one unbounded variant leaves the union unbounded"
        );
    }

    #[test]
    fn a_tag_widens_past_255_variants() {
        let tag = |count: usize| {
            let variants: Vec<String> = (0..count).map(|index| format!("V{index}")).collect();
            let (schema, wire) = compiled(&[&format!(
                "namespace a\nunion U {{ {} }}\n",
                variants.join(", ")
            )]);
            let layout = wire.union(id(&schema, "U"));
            (layout.tag, layout.size)
        };
        assert_eq!(tag(255), (Width::U8, Size::Var { max: Some(1) }));
        assert_eq!(tag(256), (Width::U16, Size::Var { max: Some(2) }));
        assert_eq!(UnionLayout::tag_of(0), 1, "0 is never a tag");
        assert_eq!(UnionLayout::tag_of(254), 255);
        assert_eq!(UnionLayout::tag_of(MAX_VARIANTS - 1), u16::MAX);
    }

    #[test]
    fn types_lay_out_as_their_target() {
        let (schema, wire, m) = message(concat!(
            "enum E: u16 { A }\n",
            "struct P { x: f32, y: f32 }\n",
            "type Level = u8[max=100]\n",
            "type Big = i64[min=0, max=70000]\n",
            "type Id = uuid\n",
            "type Hue = E\n",
            "type Alias = Level\n",
            "type Flag = bool\n",
            "message(client -> sim) M { level: Level, big: Big, id: Id, hue: Hue, alias: Alias, ",
            "ids: vec<Id>, keyed: map<Big, P>, grid: array<Level, 3> }",
        ));
        let target = |name: &str| {
            let DefKind::Type(item) = &schema.def(id(&schema, name)).kind else {
                panic!("expected a type");
            };
            item.target.clone()
        };
        for (name, bytes) in [
            ("Level", 1),
            ("Big", 4),
            ("Id", 16),
            ("Hue", 2),
            ("Alias", 1),
            ("Flag", 1),
        ] {
            let named = Type::from(TypeKind::Named(id(&schema, name)));
            assert_eq!(wire.size(&named), wire.size(&target(name)), "{name}");
            assert_eq!(wire.size(&named), Size::Fixed(bytes), "{name}");
        }
        // The target carries the narrowing.
        assert_eq!(
            Int::of(schema.resolved(field(&schema, m, "big"))),
            Some(Int::Biased {
                min: 0,
                width: Width::U32
            })
        );

        let layout = wire.layout(m);
        assert_eq!(
            layout.fields[..5],
            [
                Slot::At(0),
                Slot::At(1),
                Slot::At(5),
                Slot::At(21),
                Slot::At(23),
            ]
        );
        let sequence = |name| wire.sequence(field(&schema, m, name));
        assert_eq!(sequence("ids"), Some(Sequence::Packed { item: 16 }));
        assert_eq!(sequence("keyed"), Some(Sequence::Packed { item: 12 }));
        assert_eq!(wire.size(field(&schema, m, "grid")), Size::Fixed(3));
    }

    #[test]
    fn types_over_bool_take_bitmap_bits() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "type Flag = bool\n",
            "type Ready = Flag\n",
            "struct S { a: Flag, x: u8, b: option<Ready>, c: bool, all: array<Flag, 2> }\n",
            "union U { A { f: Flag }, B(Flag) }\n",
        )]);
        let layout = wire.layout(id(&schema, "S"));
        assert_eq!(layout.bitmap, 1);
        assert_eq!(
            layout.fields,
            [
                Slot::Bit(0),
                Slot::At(1),
                Slot::OptionalBit {
                    presence: 1,
                    value: 2
                },
                Slot::Bit(3),
                // Outside a bitmap, it takes a byte, as a `bool` does.
                Slot::At(2),
            ]
        );
        assert_eq!(layout.size, Size::Fixed(4), "`option<bool>` stays fixed");

        let union = wire.union(id(&schema, "U"));
        let a = union.variants[0].as_ref().expect("a layout");
        assert_eq!(a.fields, [Slot::Bit(0)]);
        assert_eq!(a.size, Size::Fixed(1));
        assert_eq!(union.size, Size::Var { max: Some(2) });
    }

    #[test]
    fn hub_kinds_lay_out_as_structs() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "struct S { a: bool, x: u16, name: string[max_length=8], hp: option<u32> }\n",
            "hub(u64) H { a: bool, x: u16, name: string[max_length=8], hp: option<u32> }\n",
        )]);
        assert_eq!(wire.layout(id(&schema, "H")), wire.layout(id(&schema, "S")));
    }

    #[test]
    fn components_lay_out_as_structs() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "struct S { a: bool, x: u16, name: string[max_length=8], hp: option<u32> }\n",
            "component C { a: bool, x: u16, name: string[max_length=8], hp: option<u32> }\n",
        )]);
        assert_eq!(wire.layout(id(&schema, "C")), wire.layout(id(&schema, "S")));
    }

    /// The patch of the only component, `C`, after a `namespace a` header.
    fn patch(body: &str) -> PatchLayout {
        let (schema, wire) = compiled(&[&format!("namespace a\n{body}\n")]);
        wire.patch(id(&schema, "C")).expect("a patch").clone()
    }

    fn slot(dirty: u32, some: Option<u32>, value: PatchValue) -> PatchSlot {
        PatchSlot { dirty, some, value }
    }

    #[test]
    fn patches_put_dirty_bits_first() {
        let layout = patch(concat!(
            "type Flag = bool\n",
            "struct P { x: f32 }\n",
            "@replicate(patch) component C { a: bool, hp: u16, o: option<bool>, p: option<P>, ",
            "s: option<string>, f: Flag, name: string[max_length=8], ",
            "buffs: vec<u16>[max_length=4], pos: P, n: option<Flag> }",
        ));
        assert_eq!(
            layout.fields,
            [
                slot(0, None, PatchValue::Bit(10)),
                slot(1, None, PatchValue::Packed),
                slot(2, Some(11), PatchValue::Bit(12)),
                slot(3, Some(13), PatchValue::Packed),
                slot(4, Some(14), PatchValue::Var(0)),
                slot(5, None, PatchValue::Bit(15)),
                slot(6, None, PatchValue::Var(1)),
                slot(7, None, PatchValue::Var(2)),
                slot(8, None, PatchValue::Packed),
                slot(9, Some(16), PatchValue::Bit(17)),
            ]
        );
        assert_eq!(layout.bitmap, 3, "18 bits");
        assert_eq!(layout.boundaries, 3, "no fixed fields in the head");
        assert_eq!(layout.vars, 3);
        assert_eq!(layout.boundary, Width::U16, "`s` is unbounded");
        assert_eq!(layout.head, 7, "the bitmap and two boundaries");
        assert_eq!(layout.size, Size::Var { max: None });
    }

    #[test]
    fn patches_of_fixed_components_vary_in_size() {
        let layout =
            patch("@replicate(patch) component C { hp: u32, alive: bool, pos: array<f32, 3> }");
        assert_eq!(
            layout.fields,
            [
                slot(0, None, PatchValue::Packed),
                slot(1, None, PatchValue::Bit(3)),
                slot(2, None, PatchValue::Packed),
            ]
        );
        assert_eq!(layout.head, 1, "the bitmap alone");
        assert_eq!(layout.vars, 0);
        // 1 (bitmap) + 4 (hp) + 12 (pos)
        assert_eq!(layout.size, Size::Var { max: Some(17) });
    }

    #[test]
    fn a_patch_with_one_slot_has_no_boundaries() {
        let layout = patch("@replicate(patch) component C { hp: u32, name: string }");
        assert_eq!(layout.fields[1], slot(1, None, PatchValue::Var(0)));
        assert_eq!(layout.vars, 1);
        assert_eq!(layout.head, 1, "the bitmap alone");
        assert_eq!(layout.size, Size::Var { max: None });
    }

    #[test]
    fn patch_boundaries_narrow_when_the_patch_fits() {
        let layout = |a: u32| {
            let layout = patch(&format!(
                "@replicate(patch) component C {{ hp: u32, a: string[max_length={a}], \
                 b: option<string[max_length=60]> }}"
            ));
            (layout.boundary, layout.head, layout.size)
        };
        // 1 (bitmap) + 1 (boundary) + 4 (hp) + 189 + 60
        assert_eq!(layout(189), (Width::U8, 2, Size::Var { max: Some(255) }));
        assert_eq!(layout(190), (Width::U16, 3, Size::Var { max: Some(257) }));
    }

    #[test]
    fn only_patch_components_have_patches() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "struct P { hp: u32 }\n",
            "union U { A, B }\n",
            "component R { hp: u32 }\n",
            "@replicate(ops(U)) component O { hp: u32 }\n",
            "@replicate(patch) component C { hp: u32 }\n",
        )]);
        for name in ["P", "U", "R", "O"] {
            assert_eq!(wire.patch(id(&schema, name)), None, "{name}");
        }
        assert!(wire.patch(id(&schema, "C")).is_some());
    }

    #[test]
    fn the_documented_patch_lays_out_as_documented() {
        let layout = patch(concat!(
            "@replicate(patch) component C { alive: bool, hp: u16, level: option<u8>, ",
            "title: option<string[max_length=16]>, name: string[max_length=12] }",
        ));
        assert_eq!(
            layout,
            PatchLayout {
                // 1 (bitmap) + 1 (boundary) + 2 (hp) + 1 (level) + 16 + 12
                size: Size::Var { max: Some(33) },
                bitmap: 1,
                boundaries: 1,
                boundary: Width::U8,
                head: 2,
                vars: 2,
                fields: vec![
                    slot(0, None, PatchValue::Bit(5)),
                    slot(1, None, PatchValue::Packed),
                    slot(2, Some(6), PatchValue::Packed),
                    slot(3, Some(7), PatchValue::Var(0)),
                    slot(4, None, PatchValue::Var(1)),
                ],
            }
        );
    }

    /// The errors laying out `a.C`, a `@replicate(patch)` component with
    /// `count` fields of `ty`. Built by hand, as the source would be large.
    fn patch_errors(ty: TypeKind, count: usize) -> Vec<String> {
        use crate::ir::{Component, Def, Namespace, NamespaceId, Visibility};
        let field = Field {
            name: "f".into(),
            ty: ty.into(),
            optional: false,
        };
        let schema = Schema {
            namespaces: vec![Namespace {
                name: vec!["a".into()],
            }],
            defs: vec![Def {
                namespace: NamespaceId(0),
                parent: None,
                name: "C".into(),
                kind: DefKind::Component(Component {
                    fields: vec![field; count],
                    replicate: Replicate::Patch,
                    visibility: Visibility::Public,
                    halo: true,
                }),
            }],
            consts: Vec::new(),
        };
        let mut errors = Vec::new();
        Layouts::new(&schema, &mut errors);
        errors.into_iter().map(|error| error.message).collect()
    }

    #[test]
    fn oversized_patch_heads_are_reported() {
        // A bool takes one bit in the component and two in its patch.
        assert_eq!(patch_errors(TypeKind::Bool, 262_140), [] as [String; 0]);
        assert_eq!(
            patch_errors(TypeKind::Bool, 262_144),
            ["the patch of `a.C` takes at least 65536 bytes; a message holds at most 65535"]
        );
        // The component reports itself, so its patch does not.
        assert_eq!(
            patch_errors(TypeKind::String, 33_000),
            ["`a.C` takes at least 65998 bytes; a message holds at most 65535"]
        );
    }

    #[test]
    fn oversized_values_are_reported() {
        let schema = compile(&["namespace a\nstruct S { a: array<u64, 9000> }\n"]);
        let messages: Vec<_> = schema
            .expect_err("expected errors")
            .into_iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(
            messages,
            ["`array<u64, 9000>` takes 72000 bytes; a message holds at most 65535"]
        );

        let schema =
            compile(&["namespace a\nstruct S { a: array<u64, 5000>, b: array<u64, 5000> }\n"]);
        let messages: Vec<_> = schema
            .expect_err("expected errors")
            .into_iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert_eq!(
            messages,
            ["`a.S` takes at least 80000 bytes; a message holds at most 65535"]
        );
    }
}
