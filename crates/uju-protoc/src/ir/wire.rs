//! Wire layout of a lowered [`Schema`]: where each value sits in an encoded
//! message, and how wide each integer, count, length, and offset is. Every
//! backend reads its layout from here, so all targets agree byte for byte.
//! `docs/protocol/wire-format.md` specifies the format itself.

use std::fmt::Write as _;

use crate::ir::{DefKind, Direction, Field, Number, Schema, Type, TypeId, TypeKind};

/// The largest encoded message, in bytes. Every length, count, and offset
/// therefore fits in a `u16`.
pub const MAX_SIZE: u32 = u16::MAX as u32;

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

/// Layout of a struct or message.
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

/// A message's id in each direction it travels; `bi` messages have both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ids {
    /// Among the messages a server receives: `in` and `bi`.
    pub inbound: Option<u16>,
    /// Among the messages a server sends: `out` and `bi`.
    pub outbound: Option<u16>,
}

/// A declaration that cannot be laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub def: TypeId,
    /// The field at fault, by declaration index; `None` for the whole
    /// declaration.
    pub field: Option<usize>,
    pub message: String,
}

/// The wire layout of every declaration in a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire {
    /// Indexed by [`TypeId`].
    sizes: Vec<Size>,
    /// Indexed by [`TypeId`]; `None` for enums.
    layouts: Vec<Option<Layout>>,
    /// Indexed by [`TypeId`]; empty for everything but messages.
    ids: Vec<Ids>,
    inbound: Vec<TypeId>,
    outbound: Vec<TypeId>,
    fingerprint: u64,
}

impl Wire {
    pub fn new(schema: &Schema) -> Result<Wire, Vec<Error>> {
        let len = schema.defs.len();
        let mut builder = Builder {
            schema,
            states: vec![State::Todo; len],
            layouts: vec![None; len],
            errors: Vec::new(),
        };
        let sizes: Vec<Size> = (0..len).map(|index| builder.def(TypeId(index))).collect();

        let mut messages: Vec<(String, TypeId, Direction)> = schema
            .defs
            .iter()
            .enumerate()
            .filter_map(|(index, def)| match &def.kind {
                DefKind::Message(item) => Some((
                    schema.qualified(TypeId(index)),
                    TypeId(index),
                    item.direction,
                )),
                _ => None,
            })
            .collect();
        // Qualified names are unique.
        messages.sort_by(|left, right| left.0.cmp(&right.0));
        let mut ids = vec![Ids::default(); len];
        let mut inbound = Vec::new();
        let mut outbound = Vec::new();
        for (_, id, direction) in messages {
            let mut assign = |space: &mut Vec<TypeId>, name: &str| {
                // From 1: a zeroed id is no message.
                let index = u16::try_from(space.len() + 1).ok();
                if index.is_none() {
                    builder.errors.push(Error {
                        def: id,
                        field: None,
                        message: format!("more than {} {name} messages", u16::MAX),
                    });
                }
                space.push(id);
                index
            };
            if matches!(direction, Direction::In | Direction::Bi) {
                ids[id.0].inbound = assign(&mut inbound, "inbound");
            }
            if matches!(direction, Direction::Out | Direction::Bi) {
                ids[id.0].outbound = assign(&mut outbound, "outbound");
            }
        }

        if !builder.errors.is_empty() {
            return Err(builder.errors);
        }
        Ok(Wire {
            sizes,
            layouts: builder.layouts,
            ids,
            inbound,
            outbound,
            fingerprint: fnv1a(canonical(schema).as_bytes()),
        })
    }

    pub fn size(&self, ty: &Type) -> Size {
        type_size(ty, &mut |id| self.sizes[id.0])
    }

    /// How `ty` lays out its items, when it is a `vec`, `set`, or `map`.
    pub fn sequence(&self, ty: &Type) -> Option<Sequence> {
        sequence(ty, &mut |id| self.sizes[id.0]).map(|(sequence, _)| sequence)
    }

    /// The layout of a struct or message.
    ///
    /// # Panics
    ///
    /// When `id` is an enum, which is laid out as its repr.
    pub fn layout(&self, id: TypeId) -> &Layout {
        self.layouts[id.0]
            .as_ref()
            .expect("only structs and messages have a layout")
    }

    pub fn ids(&self, id: TypeId) -> Ids {
        self.ids[id.0]
    }

    /// The messages a server receives, in inbound id order from 1.
    pub fn inbound(&self) -> &[TypeId] {
        &self.inbound
    }

    /// The messages a server sends, in outbound id order from 1.
    pub fn outbound(&self) -> &[TypeId] {
        &self.outbound
    }

    /// A hash of everything both peers must agree on, names included; peers
    /// with different fingerprints cannot talk.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
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
            DefKind::Struct(item) => self.layout(id, &item.fields),
            DefKind::Message(item) => self.layout(id, &item.fields),
        };
        self.states[id.0] = State::Done(size);
        size
    }

    fn size(&mut self, ty: &Type) -> Size {
        type_size(ty, &mut |id| self.def(id))
    }

    fn layout(&mut self, id: TypeId, fields: &[Field]) -> Size {
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
                    field: Some(index),
                    message: format!(
                        "`{}` takes {size} bytes; a message holds at most {MAX_SIZE}",
                        self.schema.type_name(&field.ty)
                    ),
                });
            }
            let slot = match (field.optional, &field.ty.kind, size) {
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
        let count = vars.saturating_sub(1) as u64;
        // Narrow boundaries when the whole object, laid out with them, fits.
        let narrow = var.map(|var| boundaries + count + optional + var);
        let boundary = Width::length(narrow.and_then(bound));
        let head = boundaries + count * boundary.bytes() as u64;

        if head > MAX_SIZE as u64 && !oversized {
            self.errors.push(Error {
                def: id,
                field: None,
                message: format!(
                    "`{}` takes at least {head} bytes; a message holds at most {MAX_SIZE}",
                    self.schema.qualified(id)
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
        self.layouts[id.0] = Some(Layout {
            size,
            bitmap,
            boundaries: boundaries.min(u32::MAX as u64) as u32,
            boundary,
            head: head.min(u32::MAX as u64) as u32,
            vars,
            fields: slots,
        });
        size
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

/// `size` as a bound on a value inside a message: `None` when the message
/// size bounds it more tightly.
fn bound(size: u64) -> Option<u32> {
    (size <= MAX_SIZE as u64).then_some(size as u32)
}

fn type_size(ty: &Type, def: &mut impl FnMut(TypeId) -> Size) -> Size {
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
        TypeKind::F64 | TypeKind::Timestamp | TypeKind::Interval | TypeKind::Entity => 8,
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
fn sequence(ty: &Type, def: &mut impl FnMut(TypeId) -> Size) -> Option<(Sequence, Size)> {
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

/// Every declaration as written in source, sorted by qualified name, so the
/// order sources are compiled in does not matter. Field order does: it is
/// the layout.
fn canonical(schema: &Schema) -> String {
    let mut defs: Vec<(String, TypeId)> = (0..schema.defs.len())
        .map(|index| (schema.qualified(TypeId(index)), TypeId(index)))
        .collect();
    defs.sort_by(|left, right| left.0.cmp(&right.0));

    let mut out = String::new();
    for (name, id) in defs {
        let fields = match &schema.def(id).kind {
            DefKind::Enum(item) => {
                let _ = write!(out, "enum {name}: {} {{", item.repr);
                for variant in &item.variants {
                    let _ = write!(out, " {} = {},", variant.name, variant.value);
                }
                out.push_str(" }\n");
                continue;
            }
            DefKind::Struct(item) => {
                let _ = write!(out, "struct {name} {{");
                &item.fields
            }
            DefKind::Message(item) => {
                let _ = write!(out, "message[{}] {name} {{", item.direction.name());
                &item.fields
            }
        };
        for field in fields {
            let ty = schema.type_name(&field.ty);
            if field.optional {
                let _ = write!(out, " {}: option<{ty}>,", field.name);
            } else {
                let _ = write!(out, " {}: {ty},", field.name);
            }
        }
        out.push_str(" }\n");
    }
    out
}

/// 64-bit FNV-1a: tiny, stable, and enough to tell schemas apart. It is not
/// collision-resistant against a deliberate attacker, and need not be.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    const EXAMPLE1: &str = include_str!("../../tests/data/example1.uju");
    const EXAMPLE2: &str = include_str!("../../tests/data/example2.uju");

    fn compiled(sources: &[&str]) -> (Schema, Wire) {
        let schema = compile(sources)
            .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
            .schema;
        let wire = Wire::new(&schema).expect("lowering checks the layout");
        (schema, wire)
    }

    fn id(schema: &Schema, name: &str) -> TypeId {
        TypeId(
            schema
                .defs
                .iter()
                .position(|def| def.name == name)
                .unwrap_or_else(|| panic!("no declaration named `{name}`")),
        )
    }

    fn fields(schema: &Schema, id: TypeId) -> &[Field] {
        match &schema.def(id).kind {
            DefKind::Struct(item) => &item.fields,
            DefKind::Message(item) => &item.fields,
            DefKind::Enum(_) => panic!("an enum has no fields"),
        }
    }

    fn field<'a>(schema: &'a Schema, id: TypeId, name: &str) -> &'a Type {
        &fields(schema, id)
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("no field named `{name}`"))
            .ty
    }

    /// The layout of the only message of `M`, after a `namespace a;` header.
    fn message(body: &str) -> (Schema, Wire, TypeId) {
        let (schema, wire) = compiled(&[&format!("namespace a;\n{body}\n")]);
        let id = id(&schema, "M");
        (schema, wire, id)
    }

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
            let (schema, _, m) = message(&format!("message[in] M {{ x: {ty} }}"));
            assert_eq!(Int::of(field(&schema, m, "x")), Some(expected), "{ty}");
        }
    }

    #[test]
    fn scalars_have_fixed_sizes() {
        let (schema, wire, m) = message(concat!(
            "enum E: u16 { A }\n",
            "message[in] M { a: f32, b: timestamp, c: interval, d: entity, e: uentity, ",
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
            "namespace a;\nstruct S { a: bool, x: u16, b: bool, y: f64, flags: array<bool, 2> }\n",
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
        let (_, wire, m) = message("message[in] M { a: string, b: string, c: string }");
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
            "message[in] M { flag: bool, o: option<bool>, p: option<P>, s: option<string>, ",
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
            "namespace a;\n",
            "struct Named { id: u32, name: string[max_length=8] }\n",
            "message[out] M { all: vec<Named>[max_length=4] }\n",
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
            "message[in] M { ps: vec<P>, flags: vec<bool>, empties: vec<Empty>[max_length=9], ",
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
    fn messages_take_dense_ids_per_direction() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a;\n",
            "message[in] C {}\n",
            "message[bi] B {}\n",
            "message[out] D {}\n",
            "message[in] A {}\n",
        )]);
        let names = |ids: &[TypeId]| -> Vec<String> {
            ids.iter().map(|id| schema.qualified(*id)).collect()
        };
        assert_eq!(names(wire.inbound()), ["a.A", "a.B", "a.C"]);
        assert_eq!(names(wire.outbound()), ["a.B", "a.D"]);
        assert_eq!(
            wire.ids(id(&schema, "B")),
            Ids {
                inbound: Some(2),
                outbound: Some(1),
            }
        );
        assert_eq!(
            wire.ids(id(&schema, "A")).inbound,
            Some(1),
            "no message is 0"
        );
        assert_eq!(wire.ids(id(&schema, "D")).inbound, None);
    }

    #[test]
    fn fingerprints_ignore_source_order() {
        let (_, forward) = compiled(&[EXAMPLE1, EXAMPLE2]);
        let (_, backward) = compiled(&[EXAMPLE2, EXAMPLE1]);
        assert_eq!(forward.fingerprint(), backward.fingerprint());
    }

    #[test]
    fn fingerprints_cover_names_layout_and_constraints() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a;\n{src}\n")])
                .1
                .fingerprint()
        };
        let base = fingerprint("message[in] M { x: u8, y: u16 }");
        assert_eq!(base, fingerprint("message[in] M { x: u8, y: u16 }"));
        for changed in [
            "message[in] M { x: u8, z: u16 }",
            "message[in] N { x: u8, y: u16 }",
            "message[in] M { y: u16, x: u8 }",
            "message[out] M { x: u8, y: u16 }",
            "message[in] M { x: u8[max=9], y: u16 }",
            "message[in] M { x: option<u8>, y: u16 }",
            "message[in] M { x: u8, y: u16 }\nenum E { A }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
    }

    #[test]
    fn oversized_values_are_reported() {
        let schema = compile(&["namespace a;\nstruct S { a: array<u64, 9000> }\n"]);
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
            compile(&["namespace a;\nstruct S { a: array<u64, 5000>, b: array<u64, 5000> }\n"]);
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
