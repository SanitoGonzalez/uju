//! Wire layout of a lowered [`Schema`]: where each value sits in an encoded
//! message, and how wide each integer, count, length, and offset is. Every
//! backend reads its layout from here, so all targets agree byte for byte.
//! `docs/protocol/wire-format.md` specifies the format itself.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::ir::{
    ConstId, ConstValue, DefKind, Field, Link, Number, Replicate, Role, Schema, Type, TypeId,
    TypeKind, VariantData, Visibility,
};

/// The largest encoded message, in bytes. Every length, count, and offset
/// therefore fits in a `u16`.
pub const MAX_SIZE: u32 = u16::MAX as u32;

/// The most variants a union holds, so its tag fits in a `u16`.
pub const MAX_VARIANTS: usize = u16::MAX as usize;

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

/// The id space of one role pair: the messages linking `from` to `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub from: Role,
    pub to: Role,
    /// In id order from 1, which is qualified name order.
    pub messages: Vec<TypeId>,
}

/// The schema's two fingerprints. Peers with different fingerprints
/// cannot talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprints {
    /// What clients see: the messages with a client endpoint, the
    /// components and tags clients see, every audience, what they all
    /// reach, and every const.
    pub client: u64,
    /// Everything.
    pub internal: u64,
}

/// A declaration that cannot be laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub def: TypeId,
    /// The union variant at fault, by declaration index.
    pub variant: Option<usize>,
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
    /// Indexed by [`TypeId`]; `Some` for structs, messages, hub kinds, and
    /// components.
    layouts: Vec<Option<Layout>>,
    /// Indexed by [`TypeId`]; `Some` for unions.
    unions: Vec<Option<UnionLayout>>,
    /// Indexed by [`TypeId`]: a message's id on each of its links, in
    /// link order. Empty for everything but messages.
    ids: Vec<Vec<u16>>,
    /// The role pairs some message links, in canonical role order.
    spaces: Vec<Space>,
    /// Indexed by [`TypeId`]; `Some` for hub kinds.
    hub_ids: Vec<Option<u16>>,
    hubs: Vec<TypeId>,
    /// Indexed by [`TypeId`]; `Some` for components and tags.
    component_ids: Vec<Option<u16>>,
    /// Components and tags in id order from 1: those clients see, then
    /// the rest.
    components: Vec<TypeId>,
    /// How many of `components` clients see.
    client_components: usize,
    /// Indexed by [`TypeId`]; `Some` for audiences.
    audience_ids: Vec<Option<u16>>,
    audiences: Vec<TypeId>,
    fingerprints: Fingerprints,
}

impl Wire {
    pub fn new(schema: &Schema) -> Result<Wire, Vec<Error>> {
        let len = schema.defs.len();
        let mut builder = Builder {
            schema,
            states: vec![State::Todo; len],
            layouts: vec![None; len],
            unions: vec![None; len],
            errors: Vec::new(),
        };
        let sizes: Vec<Size> = (0..len).map(|index| builder.def(TypeId(index))).collect();

        let mut messages: Vec<(String, TypeId, &[Link])> = schema
            .defs
            .iter()
            .enumerate()
            .filter_map(|(index, def)| match &def.kind {
                DefKind::Message(item) => Some((
                    schema.qualified(TypeId(index)),
                    TypeId(index),
                    &item.links[..],
                )),
                _ => None,
            })
            .collect();
        // Qualified names are unique.
        messages.sort_by(|left, right| left.0.cmp(&right.0));
        let mut ids = vec![Vec::new(); len];
        let mut spaces: BTreeMap<(Role, Role), Vec<TypeId>> = BTreeMap::new();
        for (_, id, links) in messages {
            for link in links {
                let space = spaces.entry((link.from, link.to)).or_default();
                space.push(id);
                // From 1: a zeroed id is no message.
                let index = u16::try_from(space.len()).unwrap_or_else(|_| {
                    builder.errors.push(Error {
                        def: id,
                        variant: None,
                        field: None,
                        message: format!(
                            "more than {} `{} -> {}` messages",
                            u16::MAX,
                            link.from,
                            link.to
                        ),
                    });
                    0
                });
                ids[id.0].push(index);
            }
        }

        let mut hubs: Vec<(String, TypeId)> = schema
            .defs
            .iter()
            .enumerate()
            .filter(|(_, def)| matches!(def.kind, DefKind::Hub(_)))
            .map(|(index, _)| (schema.qualified(TypeId(index)), TypeId(index)))
            .collect();
        hubs.sort_by(|left, right| left.0.cmp(&right.0));
        let mut hub_ids = vec![None; len];
        for (index, (_, id)) in hubs.iter().enumerate() {
            // From 1, as message ids.
            hub_ids[id.0] = u16::try_from(index + 1).ok();
            if hub_ids[id.0].is_none() {
                builder.errors.push(Error {
                    def: *id,
                    variant: None,
                    field: None,
                    message: format!("more than {} hub kinds", u16::MAX),
                });
            }
        }

        // Components and tags share one space. Those clients see come first,
        // so their ids never shift when a server-only one changes, and the
        // rest follow, so server links carry both without colliding.
        let mut components: Vec<(bool, String, TypeId)> = schema
            .defs
            .iter()
            .enumerate()
            .filter_map(|(index, def)| {
                let visibility = match &def.kind {
                    DefKind::Component(item) => item.visibility,
                    DefKind::Tag(item) => item.visibility,
                    _ => return None,
                };
                Some((
                    !visibility.reaches_clients(),
                    schema.qualified(TypeId(index)),
                    TypeId(index),
                ))
            })
            .collect();
        components.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
        let client_components = components.iter().filter(|(server, ..)| !server).count();
        let mut component_ids = vec![None; len];
        for (index, (_, _, id)) in components.iter().enumerate() {
            // From 1, as message ids.
            component_ids[id.0] = u16::try_from(index + 1).ok();
            if component_ids[id.0].is_none() {
                builder.errors.push(Error {
                    def: *id,
                    variant: None,
                    field: None,
                    message: format!("more than {} components and tags", u16::MAX),
                });
            }
        }

        let mut audiences: Vec<(String, TypeId)> = schema
            .defs
            .iter()
            .enumerate()
            .filter(|(_, def)| matches!(def.kind, DefKind::Audience(_)))
            .map(|(index, _)| (schema.qualified(TypeId(index)), TypeId(index)))
            .collect();
        audiences.sort_by(|left, right| left.0.cmp(&right.0));
        let mut audience_ids = vec![None; len];
        for (index, (_, id)) in audiences.iter().enumerate() {
            audience_ids[id.0] = u16::try_from(index + 1).ok();
            if audience_ids[id.0].is_none() {
                builder.errors.push(Error {
                    def: *id,
                    variant: None,
                    field: None,
                    message: format!("more than {} audiences", u16::MAX),
                });
            }
        }

        if !builder.errors.is_empty() {
            return Err(builder.errors);
        }
        Ok(Wire {
            sizes,
            layouts: builder.layouts,
            unions: builder.unions,
            ids,
            spaces: spaces
                .into_iter()
                .map(|((from, to), messages)| Space { from, to, messages })
                .collect(),
            hub_ids,
            hubs: hubs.into_iter().map(|(_, id)| id).collect(),
            component_ids,
            components: components.into_iter().map(|(_, _, id)| id).collect(),
            client_components,
            audience_ids,
            audiences: audiences.into_iter().map(|(_, id)| id).collect(),
            fingerprints: Fingerprints {
                client: fnv1a(canonical(schema, Scope::Client).as_bytes()),
                internal: fnv1a(canonical(schema, Scope::Internal).as_bytes()),
            },
        })
    }

    pub fn size(&self, ty: &Type) -> Size {
        type_size(ty, &mut |id| self.sizes[id.0])
    }

    /// How `ty` lays out its items, when it is a `vec`, `set`, or `map`.
    pub fn sequence(&self, ty: &Type) -> Option<Sequence> {
        sequence(ty, &mut |id| self.sizes[id.0]).map(|(sequence, _)| sequence)
    }

    /// The layout of a struct, message, hub kind, or component.
    ///
    /// # Panics
    ///
    /// When `id` is not a struct, message, hub kind, or component.
    pub fn layout(&self, id: TypeId) -> &Layout {
        self.layouts[id.0]
            .as_ref()
            .expect("only structs, messages, hub kinds, and components have a layout")
    }

    /// The layout of a union.
    ///
    /// # Panics
    ///
    /// When `id` is not a union.
    pub fn union(&self, id: TypeId) -> &UnionLayout {
        self.unions[id.0].as_ref().expect("not a union")
    }

    /// Message `id`'s id on each of its links, in the order of
    /// [`Message::links`]. Empty for anything but a message.
    ///
    /// [`Message::links`]: crate::ir::Message::links
    pub fn ids(&self, id: TypeId) -> &[u16] {
        &self.ids[id.0]
    }

    /// Message `id`'s id among the messages linking `from` to `to`, `None`
    /// when it does not link them.
    pub fn message_id(&self, id: TypeId, from: Role, to: Role) -> Option<u16> {
        self.space(from, to)
            .iter()
            .position(|other| *other == id)
            .map(|index| index as u16 + 1)
    }

    /// Every role pair some message links, in canonical role order: by
    /// `from`, then `to`.
    pub fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    /// The messages linking `from` to `to`, in id order from 1.
    pub fn space(&self, from: Role, to: Role) -> &[TypeId] {
        self.spaces
            .iter()
            .find(|space| space.from == from && space.to == to)
            .map_or(&[], |space| &space.messages)
    }

    /// The id of hub kind `id`, `None` for any other declaration.
    pub fn hub_id(&self, id: TypeId) -> Option<u16> {
        self.hub_ids[id.0]
    }

    /// Every hub kind, in id order from 1.
    pub fn hubs(&self) -> &[TypeId] {
        &self.hubs
    }

    /// The id of component or tag `id`, `None` for any other declaration.
    pub fn component_id(&self, id: TypeId) -> Option<u16> {
        self.component_ids[id.0]
    }

    /// Every component and tag, in id order from 1: those clients see,
    /// by qualified name, then those with `visibility=none`, by qualified
    /// name.
    pub fn components(&self) -> &[TypeId] {
        &self.components
    }

    /// The components and tags clients see, which take ids
    /// `1..=client_components().len()`.
    pub fn client_components(&self) -> &[TypeId] {
        &self.components[..self.client_components]
    }

    /// The id of audience `id`, `None` for any other declaration.
    pub fn audience_id(&self, id: TypeId) -> Option<u16> {
        self.audience_ids[id.0]
    }

    /// Every audience, in id order from 1.
    pub fn audiences(&self) -> &[TypeId] {
        &self.audiences
    }

    /// Hashes of what peers must agree on, names included.
    pub fn fingerprints(&self) -> Fingerprints {
        self.fingerprints
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
            DefKind::Component(item) => self.object(id, &item.fields),
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
        let count = vars.saturating_sub(1) as u64;
        // Narrow boundaries when the whole object, laid out with them, fits.
        let narrow = var.map(|var| boundaries + count + optional + var);
        let boundary = Width::length(narrow.and_then(bound));
        let head = boundaries + count * boundary.bytes() as u64;

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

/// What a fingerprint covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Client,
    Internal,
}

/// Every declaration in `scope` as written in source, sorted by qualified
/// name, so the order sources are compiled in does not matter. Field order
/// does: it is the layout. Consts follow the types, sorted the same way.
///
/// A message spells its links normalized: its pairs in canonical role
/// order, comma-separated, with kinds qualified and sending kinds sorted,
/// as `message(client -> hub<a.Party>, hub<a.Guild> | hub<a.Party> -> client)`.
/// For clients, a message lists only the links with a client end, and a hub
/// kind only its key and name, as `hub(u64) a.Party`.
///
/// Components and tags spell every attribute, defaults included, as
/// `@replicate(ops(a.Op)) @visibility(public) @halo(true) component a.C { .. }`
/// and `@visibility(a.Stealth) @halo(true) tag a.Hidden`. For clients,
/// `@halo` is left out. An audience is `audience a.Faction: a.FactionId`.
fn canonical(schema: &Schema, scope: Scope) -> String {
    let listed = match scope {
        Scope::Internal => vec![true; schema.defs.len()],
        Scope::Client => client_listed(schema),
    };
    let mut defs: Vec<(String, TypeId)> = listed
        .iter()
        .enumerate()
        .filter(|(_, listed)| **listed)
        .map(|(index, _)| (schema.qualified(TypeId(index)), TypeId(index)))
        .collect();
    defs.sort_by(|left, right| left.0.cmp(&right.0));

    let fields = |out: &mut String, fields: &[Field]| {
        out.push_str(" {");
        for field in fields {
            let ty = schema.type_name(&field.ty);
            if field.optional {
                let _ = write!(out, " {}: option<{ty}>,", field.name);
            } else {
                let _ = write!(out, " {}: {ty},", field.name);
            }
        }
        out.push_str(" }");
    };
    // `@halo` is between servers only, so clients never hash it.
    let visibility = |out: &mut String, visibility: Visibility, halo: bool| {
        let _ = match visibility {
            Visibility::Public => write!(out, "@visibility(public) "),
            Visibility::Owner => write!(out, "@visibility(owner) "),
            Visibility::None => write!(out, "@visibility(none) "),
            Visibility::Audience(id) => write!(out, "@visibility({}) ", schema.qualified(id)),
        };
        if scope == Scope::Internal {
            let _ = write!(out, "@halo({halo}) ");
        }
    };
    let mut out = String::new();
    for (name, id) in defs {
        match &schema.def(id).kind {
            DefKind::Enum(item) => {
                let _ = write!(out, "enum {name}: {} {{", item.repr);
                for variant in &item.variants {
                    let _ = write!(out, " {} = {},", variant.name, variant.value);
                }
                out.push_str(" }");
            }
            DefKind::Struct(item) => {
                let _ = write!(out, "struct {name}");
                fields(&mut out, &item.fields);
            }
            DefKind::Message(item) => {
                let links: Vec<String> = item
                    .links
                    .iter()
                    .filter(|link| scope == Scope::Internal || link.has(Role::Client))
                    .map(|link| schema.link_name(link))
                    .collect();
                let _ = write!(out, "message({}) {name}", links.join(", "));
                fields(&mut out, &item.fields);
            }
            DefKind::Hub(item) => {
                let _ = write!(out, "hub({}) {name}", schema.type_name(&item.key));
                if scope == Scope::Internal {
                    fields(&mut out, &item.fields);
                }
            }
            DefKind::Union(item) => {
                let _ = write!(out, "union {name} {{");
                for variant in &item.variants {
                    let _ = write!(out, " {}", variant.name);
                    match &variant.data {
                        VariantData::Unit => {}
                        VariantData::Type(ty) => {
                            let _ = write!(out, "({})", schema.type_name(ty));
                        }
                        VariantData::Fields(variant) => fields(&mut out, variant),
                    }
                    out.push(',');
                }
                out.push_str(" }");
            }
            DefKind::Type(item) => {
                let _ = write!(out, "type {name} = {}", schema.type_name(&item.target));
            }
            DefKind::Audience(item) => {
                let _ = write!(out, "audience {name}: {}", schema.type_name(&item.key));
            }
            DefKind::Component(item) => {
                let replicate = match item.replicate {
                    Replicate::Replace => "replace".to_string(),
                    Replicate::Patch => "patch".to_string(),
                    Replicate::Ops(op) => format!("ops({})", schema.qualified(op)),
                };
                let _ = write!(out, "@replicate({replicate}) ");
                visibility(&mut out, item.visibility, item.halo);
                let _ = write!(out, "component {name}");
                fields(&mut out, &item.fields);
            }
            DefKind::Tag(item) => {
                visibility(&mut out, item.visibility, item.halo);
                let _ = write!(out, "tag {name}");
            }
        }
        out.push('\n');
    }

    let mut consts: Vec<(String, ConstId)> = (0..schema.consts.len())
        .map(|index| (schema.const_qualified(ConstId(index)), ConstId(index)))
        .collect();
    consts.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, id) in consts {
        let item = schema.constant(id);
        let _ = write!(out, "const {name}: {} = ", schema.kind_name(&item.ty));
        match &item.value {
            // Escaped by hand, so the hash never follows `{:?}`'s choices.
            ConstValue::String(value) => {
                out.push('"');
                for ch in value.chars() {
                    if matches!(ch, '"' | '\\') {
                        out.push('\\');
                    }
                    out.push(ch);
                }
                out.push('"');
            }
            value => {
                let _ = write!(out, "{value}");
            }
        }
        out.push('\n');
    }
    out
}

/// Which declarations the client fingerprint lists: every message with a
/// client end, each hub kind they name, with what its key reaches, every
/// component and tag clients see, every audience, and whatever they all
/// reach. Hub kinds are listed by key and name.
fn client_listed(schema: &Schema) -> Vec<bool> {
    let mut listed = vec![false; schema.defs.len()];
    let mut todo = Vec::new();
    for (index, def) in schema.defs.iter().enumerate() {
        let visibility = match &def.kind {
            DefKind::Audience(_) => Some(Visibility::Public),
            DefKind::Component(item) => Some(item.visibility),
            DefKind::Tag(item) => Some(item.visibility),
            _ => None,
        };
        if let Some(visibility) = visibility {
            if visibility.reaches_clients() {
                todo.push(TypeId(index));
            }
            continue;
        }
        let DefKind::Message(item) = &def.kind else {
            continue;
        };
        let links: Vec<&Link> = item
            .links
            .iter()
            .filter(|link| link.has(Role::Client))
            .collect();
        if links.is_empty() {
            continue;
        }
        todo.push(TypeId(index));
        for link in links {
            todo.extend(link.from_kinds.iter().flatten().copied());
            todo.extend(link.to_kind);
        }
    }
    while let Some(id) = todo.pop() {
        if listed[id.0] {
            continue;
        }
        listed[id.0] = true;
        let mut types: Vec<&Type> = Vec::new();
        match &schema.def(id).kind {
            DefKind::Hub(item) => types.push(&item.key),
            DefKind::Enum(_) => {}
            DefKind::Struct(item) => types.extend(item.fields.iter().map(|field| &field.ty)),
            DefKind::Message(item) => types.extend(item.fields.iter().map(|field| &field.ty)),
            DefKind::Type(item) => types.push(&item.target),
            DefKind::Union(item) => {
                for variant in &item.variants {
                    match &variant.data {
                        VariantData::Unit => {}
                        VariantData::Type(ty) => types.push(ty),
                        VariantData::Fields(fields) => {
                            types.extend(fields.iter().map(|field| &field.ty));
                        }
                    }
                }
            }
            DefKind::Audience(item) => types.push(&item.key),
            DefKind::Component(item) => {
                types.extend(item.fields.iter().map(|field| &field.ty));
                if let Replicate::Ops(op) = item.replicate {
                    todo.push(op);
                }
                if let Visibility::Audience(audience) = item.visibility {
                    todo.push(audience);
                }
            }
            DefKind::Tag(item) => {
                if let Visibility::Audience(audience) = item.visibility {
                    todo.push(audience);
                }
            }
        }
        for ty in types {
            named(ty, &mut todo);
        }
    }
    listed
}

/// The declarations `ty` names, through collections.
fn named(ty: &Type, out: &mut Vec<TypeId>) {
    match &ty.kind {
        TypeKind::Array(item, _) | TypeKind::Vec(item) | TypeKind::Set(item) => named(item, out),
        TypeKind::Map(key, value) => {
            named(key, out);
            named(value, out);
        }
        TypeKind::Named(id) => out.push(*id),
        _ => {}
    }
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
            DefKind::Hub(item) => &item.fields,
            DefKind::Component(item) => &item.fields,
            DefKind::Enum(_)
            | DefKind::Union(_)
            | DefKind::Type(_)
            | DefKind::Audience(_)
            | DefKind::Tag(_) => {
                panic!("only structs, messages, hub kinds, and components have fields")
            }
        }
    }

    fn field<'a>(schema: &'a Schema, id: TypeId, name: &str) -> &'a Type {
        &fields(schema, id)
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("no field named `{name}`"))
            .ty
    }

    /// The layout of the only message of `M`, after a `namespace a` header.
    fn message(body: &str) -> (Schema, Wire, TypeId) {
        let (schema, wire) = compiled(&[&format!("namespace a\n{body}\n")]);
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
    fn canonical_text_spells_out_unions_and_types() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "type PartyId = u64\n",
            "union Op { Add { id: PartyId, note: option<string> }, Remove(u8[max=9]), Clear }\n",
        )]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "union a.Op { Add { id: a.PartyId, note: option<string>, }, Remove(u8[max=9]), Clear, }\n",
                "type a.PartyId = u64\n",
            )
        );
    }

    #[test]
    fn canonical_text_spells_times_by_value() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "const D: duration = 1000ms\n",
            "const L: duration = 90m\n",
            "const O: duration = 1001ms\n",
            "const T: timestamp = 2026-11-07T12:00:00.000000Z\n",
            "const U: timestamp = 2026-11-07T12:00:00.250000Z\n",
            "const Z: duration = 0h\n",
        )]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "const a.D: duration = 1s\n",
                "const a.L: duration = 90m\n",
                "const a.O: duration = 1001ms\n",
                "const a.T: timestamp = 2026-11-07T12:00:00Z\n",
                "const a.U: timestamp = 2026-11-07T12:00:00.250Z\n",
                "const a.Z: duration = 0s\n",
            )
        );
    }

    #[test]
    fn fingerprints_cover_unions_and_types() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint(concat!(
            "type Id = u32\n",
            "union U { A { x: Id }, B(u8), C }\n",
        ));
        assert_eq!(
            base,
            fingerprint(concat!(
                "union U { A { x: Id, }, B(u8), C, }\n",
                "type Id = u32\n",
            )),
            "declaration order and trailing commas do not matter"
        );
        for changed in [
            // Variants: order, names, and forms.
            "type Id = u32\nunion U { B(u8), A { x: Id }, C }",
            "type Id = u32\nunion U { A { x: Id }, B(u8), D }",
            "type Id = u32\nunion U { A { y: Id }, B(u8), C }",
            "type Id = u32\nunion U { A(Id), B(u8), C }",
            "type Id = u32\nunion U { A { x: Id }, B(u16), C }",
            "type Id = u32\nunion U { A { x: Id }, B(u8[max=9]), C }",
            "type Id = u32\nunion U { A { x: Id }, B(u8), C, D }",
            "type Id = u32\nunion V { A { x: Id }, B(u8), C }",
            // A type's target and name.
            "type Id = u64\nunion U { A { x: Id }, B(u8), C }",
            "type Id = u32[max=9]\nunion U { A { x: Id }, B(u8), C }",
            "type Key = u32\nunion U { A { x: Key }, B(u8), C }",
            // A type, or the scalar it stands for.
            "type Id = u32\nunion U { A { x: u32 }, B(u8), C }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
    }

    #[test]
    fn messages_take_dense_ids_per_role_pair() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "hub(u64) P {}\n",
            "message(client -> sim) C {}\n",
            "message(client <-> sim) B {}\n",
            "message(sim | hub -> client) D {}\n",
            "message(client -> sim | hub<P>) A {}\n",
            "message(sim <-> sim) E {}\n",
        )]);
        let spaces: Vec<(Role, Role, Vec<String>)> = wire
            .spaces()
            .iter()
            .map(|space| {
                let names = space.messages.iter().map(|id| schema.qualified(*id));
                (space.from, space.to, names.collect())
            })
            .collect();
        assert_eq!(
            spaces,
            [
                (
                    Role::Client,
                    Role::Sim,
                    vec!["a.A".into(), "a.B".into(), "a.C".into()]
                ),
                (Role::Client, Role::Hub, vec!["a.A".into()]),
                (Role::Sim, Role::Client, vec!["a.B".into(), "a.D".into()]),
                (Role::Sim, Role::Sim, vec!["a.E".into()]),
                (Role::Hub, Role::Client, vec!["a.D".into()]),
            ]
        );
        let b = id(&schema, "B");
        assert_eq!(wire.ids(b), [2, 1], "in link order");
        assert_eq!(wire.message_id(b, Role::Client, Role::Sim), Some(2));
        assert_eq!(wire.message_id(b, Role::Sim, Role::Client), Some(1));
        assert_eq!(wire.message_id(b, Role::Hub, Role::Client), None);
        assert_eq!(wire.ids(id(&schema, "A")), [1, 1], "no message is 0");
        assert_eq!(wire.ids(id(&schema, "E")), [1], "`sim <-> sim` is one pair");
        assert!(wire.space(Role::Entry, Role::Client).is_empty());
        assert!(wire.ids(id(&schema, "P")).is_empty());
    }

    #[test]
    fn canonical_text_normalizes_links() {
        let canonical_of = |src: &str| {
            let (schema, _) = compiled(&[&format!(
                "namespace a\nhub(u64) Party {{}}\nhub(u32) Guild {{}}\n{src}\n"
            )]);
            let text = canonical(&schema, Scope::Internal);
            let line = text.lines().find(|line| line.starts_with("message"));
            line.unwrap().to_string()
        };
        assert_eq!(
            canonical_of("message(hub<Party> | hub<Guild> -> client) M {}"),
            "message(hub<a.Guild> | hub<a.Party> -> client) a.M { }",
            "sending kinds are sorted by qualified name"
        );
        assert_eq!(
            canonical_of("message(client <-> hub<Party>) M {}"),
            "message(client -> hub<a.Party>, hub<a.Party> -> client) a.M { }"
        );
        assert_eq!(
            canonical_of("message(hub<Party> <-> client) M {}"),
            canonical_of("message(client <-> hub<Party>) M {}")
        );
        assert_eq!(
            canonical_of("message(hub | sim -> entry | client) M {}"),
            "message(sim -> client, sim -> entry, hub -> client, hub -> entry) a.M { }"
        );
        assert_eq!(
            canonical_of("message(sim <-> sim) M {}"),
            "message(sim -> sim) a.M { }"
        );
    }

    #[test]
    fn the_client_fingerprint_lists_what_clients_see() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "type PartyId = u64\n",
            "type Exp = u32\n",
            "struct Seen { e: E }\n",
            "struct Unseen { x: u8 }\n",
            "enum E { A }\n",
            "hub(PartyId) Party { members: vec<u64> }\n",
            "hub(u32) Guild { name: string }\n",
            "hub(u16) Bank { gold: u64 }\n",
            "message(client -> hub<Party> | sim) Join { seen: Seen }\n",
            "message(hub<Guild> -> client | sim) Notice {}\n",
            "message(sim -> hub<Bank>) Deposit { exp: Exp }\n",
            "const C: u8 = 1\n",
        )]);
        // Kinds by header only, and only the links with a client end.
        assert_eq!(
            canonical(&schema, Scope::Client),
            concat!(
                "enum a.E: u32 { A = 0, }\n",
                "hub(u32) a.Guild\n",
                "message(client -> sim, client -> hub<a.Party>) a.Join { seen: a.Seen, }\n",
                "message(hub<a.Guild> -> client) a.Notice { }\n",
                "hub(a.PartyId) a.Party\n",
                "type a.PartyId = u64\n",
                "struct a.Seen { e: a.E, }\n",
                "const a.C: u8 = 1\n",
            )
        );
    }

    #[test]
    fn server_links_leave_the_client_fingerprint_alone() {
        let fingerprints = |src: &str| {
            compiled(&[&format!(
                "namespace a\nhub(u64) P {{ x: u8 }}\nmessage(client -> sim) M {{}}\n{src}\n"
            )])
            .1
            .fingerprints()
        };
        let base = fingerprints("message(sim -> hub<P>) N { x: u8 }");
        for changed in [
            "message(sim -> hub<P>) N { x: u16 }",
            "message(sim -> sim) N { x: u8 }",
            "message(hub -> sim) N { x: u8 }",
            "hub(u8) Q {}\nmessage(sim -> hub<P>) N { x: u8 }",
        ] {
            let fingerprints = fingerprints(changed);
            assert_eq!(base.client, fingerprints.client, "{changed}");
            assert_ne!(base.internal, fingerprints.internal, "{changed}");
        }
        for changed in [
            "message(client -> sim) N { x: u8 }",
            "message(sim -> client) N { x: u8 }",
            "message(sim | client -> hub<P>) N { x: u8 }",
            "const C: u8 = 1\nmessage(sim -> hub<P>) N { x: u8 }",
        ] {
            assert_ne!(base.client, fingerprints(changed).client, "{changed}");
        }
        // A kind's body is never in the client fingerprint, its key is.
        let with_kind = |kind: &str| {
            compiled(&[&format!(
                "namespace a\n{kind}\nmessage(client -> hub<P>) M {{}}\n"
            )])
            .1
            .fingerprints()
        };
        let base = with_kind("hub(u64) P { x: u8 }");
        assert_eq!(base.client, with_kind("hub(u64) P { y: u16 }").client);
        assert_ne!(base.internal, with_kind("hub(u64) P { y: u16 }").internal);
        assert_ne!(base.client, with_kind("hub(u32) P { x: u8 }").client);
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
    fn hub_kinds_take_dense_ids_by_qualified_name() {
        let (schema, wire) = compiled(&[
            "namespace b\nhub(u64) Guild {}\nhub(u32) Bank {}\n",
            "namespace a\nhub(u64) Party {}\nstruct S {}\n",
        ]);
        let names: Vec<String> = wire.hubs().iter().map(|id| schema.qualified(*id)).collect();
        assert_eq!(names, ["a.Party", "b.Bank", "b.Guild"]);
        assert_eq!(wire.hub_id(id(&schema, "Party")), Some(1), "no kind is 0");
        assert_eq!(wire.hub_id(id(&schema, "Bank")), Some(2));
        assert_eq!(wire.hub_id(id(&schema, "Guild")), Some(3));
        assert_eq!(wire.hub_id(id(&schema, "S")), None);

        let (_, wire) = compiled(&["namespace a\nstruct S {}\n"]);
        assert!(wire.hubs().is_empty());
    }

    #[test]
    fn canonical_text_spells_out_hub_kinds() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "type GuildId = u64\n",
            "hub(GuildId) Guild { enum Rank { A } name: string[max_length=24], rank: option<Rank> }\n",
            "hub(u32[max=9]) Plot {}\n",
        )]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "hub(a.GuildId) a.Guild { name: string[max_length=24], rank: option<a.Guild.Rank>, }\n",
                "enum a.Guild.Rank: u32 { A = 0, }\n",
                "type a.GuildId = u64\n",
                "hub(u32[max=9]) a.Plot { }\n",
            )
        );
    }

    #[test]
    fn fingerprints_cover_hub_kinds() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint("type Id = u64\nhub(Id) G { x: u8 }");
        assert_eq!(base, fingerprint("hub(Id) G { x: u8, }\ntype Id = u64"));
        for changed in [
            // The key, by name and by type.
            "type Id = u64\nhub(u64) G { x: u8 }",
            "type Id = u32\nhub(Id) G { x: u8 }",
            "type Key = u64\nhub(Key) G { x: u8 }",
            // The kind's name and fields.
            "type Id = u64\nhub(Id) H { x: u8 }",
            "type Id = u64\nhub(Id) G { y: u8 }",
            // A struct is not a kind.
            "type Id = u64\nstruct G { x: u8 }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
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

    #[test]
    fn components_and_tags_share_ids_with_client_ones_first() {
        let (schema, wire) = compiled(&[
            concat!(
                "namespace b\n",
                "audience Stealth: u8\n",
                "@visibility(none) component Cooldowns { x: u8 }\n",
                "@visibility(Stealth) tag Hidden\n",
                "@visibility(owner) component Bag { x: u8 }\n",
            ),
            concat!(
                "namespace a\n",
                "audience Faction: u32\n",
                "@visibility(none) tag Busy\n",
                "tag Dead\n",
                "component Health { hp: u32 }\n",
                "struct S {}\n",
            ),
        ]);
        let names = |ids: &[TypeId]| -> Vec<String> {
            ids.iter().map(|id| schema.qualified(*id)).collect()
        };
        assert_eq!(
            names(wire.components()),
            [
                "a.Dead",
                "a.Health",
                "b.Bag",
                "b.Hidden",
                "a.Busy",
                "b.Cooldowns"
            ]
        );
        assert_eq!(
            names(wire.client_components()),
            ["a.Dead", "a.Health", "b.Bag", "b.Hidden"]
        );
        for (index, id) in wire.components().iter().enumerate() {
            assert_eq!(wire.component_id(*id), Some(index as u16 + 1), "no id is 0");
        }
        assert_eq!(wire.component_id(id(&schema, "S")), None);
        assert_eq!(wire.component_id(id(&schema, "Faction")), None);

        assert_eq!(names(wire.audiences()), ["a.Faction", "b.Stealth"]);
        assert_eq!(wire.audience_id(id(&schema, "Faction")), Some(1));
        assert_eq!(wire.audience_id(id(&schema, "Stealth")), Some(2));
        assert_eq!(wire.audience_id(id(&schema, "Dead")), None);

        // A server-only component never shifts a client one's id.
        let (schema, wire) = compiled(&["namespace a\n@visibility(none) tag A\ntag B\n"]);
        assert_eq!(wire.component_id(id(&schema, "B")), Some(1));
        assert_eq!(wire.component_id(id(&schema, "A")), Some(2));

        let (_, wire) = compiled(&["namespace a\nstruct S {}\n"]);
        assert!(wire.components().is_empty());
        assert!(wire.audiences().is_empty());
    }

    const COMPONENTS: &str = concat!(
        "namespace a\n",
        "type FactionId = u32\n",
        "audience Faction: FactionId\n",
        "audience Stealth: u8\n",
        "struct Item { id: u32 }\n",
        "struct Secret { x: u8 }\n",
        "union Op { Add(Item), Clear }\n",
        "component Health { hp: u32 }\n",
        "@replicate(ops(Op)) @visibility(owner) @halo(false)\n",
        "component Inventory { items: vec<Item> }\n",
        "@replicate(patch) @visibility(none)\n",
        "component Brain { secret: Secret }\n",
        "@visibility(Faction) component Chatter { x: u8 }\n",
        "@visibility(Stealth) tag Hidden\n",
        "@visibility(none) tag Busy\n",
    );

    #[test]
    fn canonical_text_spells_out_components_tags_and_audiences() {
        let (schema, _) = compiled(&[COMPONENTS]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "@replicate(patch) @visibility(none) @halo(false) component a.Brain { secret: a.Secret, }\n",
                "@visibility(none) @halo(false) tag a.Busy\n",
                "@replicate(replace) @visibility(a.Faction) @halo(true) component a.Chatter { x: u8, }\n",
                "audience a.Faction: a.FactionId\n",
                "type a.FactionId = u32\n",
                "@replicate(replace) @visibility(public) @halo(true) component a.Health { hp: u32, }\n",
                "@visibility(a.Stealth) @halo(true) tag a.Hidden\n",
                "@replicate(ops(a.Op)) @visibility(owner) @halo(false) component a.Inventory { items: vec<a.Item>, }\n",
                "struct a.Item { id: u32, }\n",
                "union a.Op { Add(a.Item), Clear, }\n",
                "struct a.Secret { x: u8, }\n",
                "audience a.Stealth: u8\n",
            )
        );
        // Clients see no `@halo`, nothing with `visibility=none`, and nothing
        // only those reach.
        assert_eq!(
            canonical(&schema, Scope::Client),
            concat!(
                "@replicate(replace) @visibility(a.Faction) component a.Chatter { x: u8, }\n",
                "audience a.Faction: a.FactionId\n",
                "type a.FactionId = u32\n",
                "@replicate(replace) @visibility(public) component a.Health { hp: u32, }\n",
                "@visibility(a.Stealth) tag a.Hidden\n",
                "@replicate(ops(a.Op)) @visibility(owner) component a.Inventory { items: vec<a.Item>, }\n",
                "struct a.Item { id: u32, }\n",
                "union a.Op { Add(a.Item), Clear, }\n",
                "audience a.Stealth: u8\n",
            )
        );
    }

    #[test]
    fn every_audience_is_in_the_client_fingerprint() {
        let (schema, _) = compiled(&["namespace a\nstruct K {}\naudience Unused: u8\n"]);
        assert_eq!(canonical(&schema, Scope::Client), "audience a.Unused: u8\n");
    }

    #[test]
    fn fingerprints_cover_components_tags_and_audiences() {
        let fingerprints = |src: &str| {
            compiled(&[&format!("{COMPONENTS}{src}\n")])
                .1
                .fingerprints()
        };
        let base = fingerprints("");
        // Server-only changes leave clients alone.
        for changed in [
            "@visibility(none) component Extra { x: u8 }",
            "@visibility(none) tag Extra",
        ] {
            let changed_fingerprints = fingerprints(changed);
            assert_eq!(base.client, changed_fingerprints.client, "{changed}");
            assert_ne!(base.internal, changed_fingerprints.internal, "{changed}");
        }
        let swap = |from: &str, to: &str| {
            assert!(COMPONENTS.contains(from), "{from}");
            compiled(&[&COMPONENTS.replace(from, to)]).1.fingerprints()
        };
        for (from, to) in [
            // Halo, and what only a `visibility=none` component reaches.
            ("@halo(false)\n", "\n"),
            ("component Health", "@halo(false) component Health"),
            ("struct Secret { x: u8 }", "struct Secret { x: u16 }"),
            (
                "component Brain { secret: Secret }",
                "component Brain { secret: Secret, y: u8 }",
            ),
            ("@visibility(none) tag Busy", "@visibility(none) tag Idle"),
            ("@replicate(patch) @visibility(none)", "@visibility(none)"),
        ] {
            let changed = swap(from, to);
            assert_eq!(base.client, changed.client, "{from} -> {to}");
            assert_ne!(base.internal, changed.internal, "{from} -> {to}");
        }
        // What clients see changes both.
        for (from, to) in [
            (
                "component Health { hp: u32 }",
                "component Health { hp: u16 }",
            ),
            ("component Health", "@replicate(patch) component Health"),
            ("component Health", "@visibility(owner) component Health"),
            ("@visibility(Stealth) tag Hidden", "tag Hidden"),
            ("@visibility(Faction)", "@visibility(Stealth)"),
            ("audience Stealth: u8", "audience Stealth: u16"),
            ("type FactionId = u32", "type FactionId = u64"),
            ("union Op { Add(Item), Clear }", "union Op { Add(Item) }"),
            ("struct Item { id: u32 }", "struct Item { id: u64 }"),
            ("@visibility(none) tag Busy", "tag Busy"),
            ("@visibility(none) tag Busy", "@visibility(public) tag Busy"),
        ] {
            let changed = swap(from, to);
            assert_ne!(base.client, changed.client, "{from} -> {to}");
            assert_ne!(base.internal, changed.internal, "{from} -> {to}");
        }
        // Defaults hash as if spelled out.
        assert_eq!(
            base,
            swap(
                "component Health",
                "@replicate(replace) @visibility(public) @halo(true) component Health"
            )
        );
        assert_eq!(
            base,
            swap(
                "@visibility(none) tag Busy",
                "@visibility(none) @halo(false) tag Busy"
            )
        );
    }

    #[test]
    fn fingerprints_ignore_source_order() {
        let (_, forward) = compiled(&[EXAMPLE1, EXAMPLE2]);
        let (_, backward) = compiled(&[EXAMPLE2, EXAMPLE1]);
        assert_eq!(forward.fingerprints(), backward.fingerprints());
    }

    #[test]
    fn fingerprints_cover_names_layout_and_constraints() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint("message(client -> sim) M { x: u8, y: u16 }");
        assert_eq!(
            base,
            fingerprint("message(client -> sim) M { x: u8, y: u16 }")
        );
        for changed in [
            "message(client -> sim) M { x: u8, z: u16 }",
            "message(client -> sim) N { x: u8, y: u16 }",
            "message(client -> sim) M { y: u16, x: u8 }",
            "message(sim -> client) M { x: u8, y: u16 }",
            "message(client -> sim) M { x: u8[max=9], y: u16 }",
            "message(client -> sim) M { x: option<u8>, y: u16 }",
            "message(client -> sim) M { x: u8, y: u16 }\nenum E { A }",
            "message(client -> sim) M { x: u8, y: u16 }\nconst C: u8 = 1",
            "message(client -> sim) M { const C: u8 = 1 x: u8, y: u16 }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
    }

    #[test]
    fn fingerprints_cover_const_values() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint("const C: u8 = 1\nconst S: string = \"a\"");
        for changed in [
            "const C: u8 = 2\nconst S: string = \"a\"",
            "const C: u16 = 1\nconst S: string = \"a\"",
            "const D: u8 = 1\nconst S: string = \"a\"",
            "const C: u8 = 1\nconst S: string = \"b\"",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
        // Unescaped, the first would read exactly as the second.
        assert_ne!(
            fingerprint(r#"const A: string = "x\"\nconst a.B: string = \"y""#),
            fingerprint("const A: string = \"x\"\nconst B: string = \"y\""),
        );

        // Durations and timestamps count by value, not by spelling.
        assert_eq!(
            fingerprint("const D: duration = 1000ms"),
            fingerprint("const D: duration = 1s")
        );
        assert_ne!(
            fingerprint("const D: duration = 1s"),
            fingerprint("const D: duration = 2s")
        );
        assert_eq!(
            fingerprint("const T: timestamp = 2026-01-01T00:00:00.000Z"),
            fingerprint("const T: timestamp = 2026-01-01T00:00:00Z")
        );
        assert_ne!(
            fingerprint("const T: timestamp = 2026-01-01T00:00:00Z"),
            fingerprint("const T: timestamp = 2026-01-01T00:00:01Z")
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
