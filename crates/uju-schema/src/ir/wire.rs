//! Wire-format layout computed from the IR; the format itself is specified
//! in `docs/schema/wire.md`. [`Wire::new`] resolves every declaration to
//! offsets, sizes, and alignments, from which backends emit encoders and
//! decoders and pre-calculate encoded sizes.

use std::cmp::Reverse;

use crate::ir::{DefKind, Message, Schema, Type, TypeId, Union};

/// Size and alignment of a fixed-size type. The size is always a multiple of
/// the alignment, so it is also the stride inside containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: usize,
    pub align: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire {
    /// Parallel to [`Schema::defs`].
    pub defs: Vec<DefWire>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefWire {
    Enum(Layout),
    Struct(StructWire),
    Union(UnionWire),
    Message(MessageWire),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructWire {
    pub layout: Layout,
    /// Byte offset of each field, parallel to the struct's fields.
    pub offsets: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnionWire {
    /// Alignment of the encoded member following the u16 tag: the maximum
    /// over all members, so the padding is the same whichever is set.
    pub payload_align: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageWire {
    pub align: usize,
    /// Byte length of the fixed section; payloads follow, each padded to its
    /// alignment, in field declaration order.
    pub fixed_size: usize,
    pub presence_offset: usize,
    pub presence_bytes: usize,
    /// Parallel to the message's fields.
    pub fields: Vec<FieldWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldWire {
    /// Offset of the inline value (fixed-size field) or of the u16 slot
    /// (variable-size field).
    pub offset: usize,
    /// Presence bit index of an optional field, LSB-first from
    /// [`MessageWire::presence_offset`].
    pub presence: Option<u32>,
}

/// Layout of one `map` entry whose value type is fixed-size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryLayout {
    pub layout: Layout,
    pub key_offset: usize,
    pub value_offset: usize,
}

impl Wire {
    pub fn new(schema: &Schema) -> Wire {
        let mut builder = Builder {
            schema,
            slots: schema.defs.iter().map(|_| Slot::Empty).collect(),
        };
        for (index, def) in schema.defs.iter().enumerate() {
            if !matches!(def.kind, DefKind::Message(_)) {
                builder.def_wire(TypeId(index));
            }
        }
        // Union alignments may depend on each other; iterate to the fixpoint
        // (alignments only grow, bounded by 8).
        loop {
            let mut changed = false;
            for (index, def) in schema.defs.iter().enumerate() {
                if let DefKind::Union(union) = &def.kind {
                    let align = union_align(&mut builder, union);
                    if let Slot::Done(DefWire::Union(wire)) = &mut builder.slots[index]
                        && wire.payload_align != align
                    {
                        wire.payload_align = align;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (index, def) in schema.defs.iter().enumerate() {
            if matches!(def.kind, DefKind::Message(_)) {
                builder.def_wire(TypeId(index));
            }
        }
        Wire {
            defs: builder
                .slots
                .into_iter()
                .map(|slot| match slot {
                    Slot::Done(def) => def,
                    _ => unreachable!("every declaration was computed"),
                })
                .collect(),
        }
    }

    pub fn def(&self, id: TypeId) -> &DefWire {
        &self.defs[id.0]
    }

    pub fn layout(&self, ty: &Type) -> Layout {
        layout(&mut Complete(&self.defs), ty)
    }

    pub fn is_fixed(&self, ty: &Type) -> bool {
        is_fixed(&mut Complete(&self.defs), ty)
    }

    /// Alignment of a variable-size field's payload in the variable section.
    pub fn payload_align(&self, ty: &Type) -> usize {
        payload_align(&mut Complete(&self.defs), ty)
    }

    /// Alignment of a variable-size element's `[u16 head][padding][payload]`
    /// block inside a container.
    pub fn block_align(&self, ty: &Type) -> usize {
        block_align(&mut Complete(&self.defs), ty)
    }

    pub fn entry_layout(&self, key: &Type, value: &Type) -> EntryLayout {
        entry_layout(&mut Complete(&self.defs), key, value)
    }
}

trait Defs {
    fn def_wire(&mut self, id: TypeId) -> &DefWire;
}

enum Slot {
    Empty,
    Computing,
    Done(DefWire),
}

struct Builder<'a> {
    schema: &'a Schema,
    slots: Vec<Slot>,
}

impl Defs for Builder<'_> {
    fn def_wire(&mut self, id: TypeId) -> &DefWire {
        if matches!(self.slots[id.0], Slot::Empty) {
            self.compute(id);
        }
        match &self.slots[id.0] {
            Slot::Done(def) => def,
            _ => unreachable!("the IR is validated to have no fixed-size cycles"),
        }
    }
}

impl Builder<'_> {
    fn compute(&mut self, id: TypeId) {
        let schema = self.schema;
        self.slots[id.0] = Slot::Computing;
        let def = match &schema.defs[id.0].kind {
            DefKind::Enum(item) => DefWire::Enum(Layout {
                size: item.repr.size(),
                align: item.repr.size(),
            }),
            DefKind::Struct(item) => {
                let layouts: Vec<Layout> = item
                    .fields
                    .iter()
                    .map(|field| layout(self, &field.ty))
                    .collect();
                let (layout, offsets) = record(&layouts);
                DefWire::Struct(StructWire { layout, offsets })
            }
            DefKind::Union(item) => {
                // Seed the minimum so a union reachable from its own members
                // resolves; the fixpoint loop settles chains.
                self.slots[id.0] = Slot::Done(DefWire::Union(UnionWire { payload_align: 2 }));
                DefWire::Union(UnionWire {
                    payload_align: union_align(self, item),
                })
            }
            DefKind::Message(item) => DefWire::Message(message_wire(self, item)),
        };
        self.slots[id.0] = Slot::Done(def);
    }
}

struct Complete<'a>(&'a [DefWire]);

impl Defs for Complete<'_> {
    fn def_wire(&mut self, id: TypeId) -> &DefWire {
        &self.0[id.0]
    }
}

fn layout(defs: &mut impl Defs, ty: &Type) -> Layout {
    let (size, align) = match ty {
        Type::I8 | Type::U8 | Type::Bool => (1, 1),
        Type::I16 | Type::U16 => (2, 2),
        Type::I32 | Type::U32 | Type::F32 => (4, 4),
        Type::I64 | Type::U64 | Type::F64 => (8, 8),
        Type::Timestamp | Type::Interval | Type::Entity => (8, 8),
        Type::UEntity => (12, 4),
        Type::Uuid => (16, 1),
        Type::Array(item, len) => {
            let item = layout(defs, item);
            (item.size * *len as usize, item.align)
        }
        Type::Named(id) => match defs.def_wire(*id) {
            DefWire::Enum(layout) => return *layout,
            DefWire::Struct(item) => return item.layout,
            _ => panic!("`{ty:?}` is not fixed-size"),
        },
        _ => panic!("`{ty:?}` is not fixed-size"),
    };
    Layout { size, align }
}

fn is_fixed(defs: &mut impl Defs, ty: &Type) -> bool {
    match ty {
        Type::String | Type::Bytes | Type::Vec(_) | Type::Set(_) | Type::Map(..) => false,
        Type::Named(id) => matches!(defs.def_wire(*id), DefWire::Enum(_) | DefWire::Struct(_)),
        _ => true,
    }
}

fn payload_align(defs: &mut impl Defs, ty: &Type) -> usize {
    match ty {
        Type::String | Type::Bytes => 1,
        Type::Vec(item) | Type::Set(item) => elem_align(defs, item),
        Type::Map(key, value) => {
            if is_fixed(defs, value) {
                entry_layout(defs, key, value).layout.align
            } else {
                layout(defs, key).align.max(block_align(defs, value))
            }
        }
        Type::Named(id) => match defs.def_wire(*id) {
            DefWire::Union(union) => union.payload_align,
            _ => panic!("`{ty:?}` is not variable-size"),
        },
        _ => panic!("`{ty:?}` is not variable-size"),
    }
}

fn block_align(defs: &mut impl Defs, ty: &Type) -> usize {
    payload_align(defs, ty).max(2)
}

fn elem_align(defs: &mut impl Defs, ty: &Type) -> usize {
    if is_fixed(defs, ty) {
        layout(defs, ty).align
    } else {
        block_align(defs, ty)
    }
}

fn union_align(defs: &mut impl Defs, union: &Union) -> usize {
    union
        .members
        .iter()
        .map(|member| elem_align(defs, &member.ty))
        .max()
        .unwrap_or(1)
}

fn entry_layout(defs: &mut impl Defs, key: &Type, value: &Type) -> EntryLayout {
    let (layout, offsets) = record(&[layout(defs, key), layout(defs, value)]);
    EntryLayout {
        layout,
        key_offset: offsets[0],
        value_offset: offsets[1],
    }
}

/// Pack `fields` sorted by descending alignment (index breaks ties), padding
/// the total size to the alignment.
fn record(fields: &[Layout]) -> (Layout, Vec<usize>) {
    let mut order: Vec<usize> = (0..fields.len()).collect();
    order.sort_by_key(|&index| (Reverse(fields[index].align), index));

    let mut offsets = vec![0; fields.len()];
    let mut offset = 0usize;
    let mut align = 1usize;
    for index in order {
        let field = fields[index];
        offset = offset.next_multiple_of(field.align);
        offsets[index] = offset;
        offset += field.size;
        align = align.max(field.align);
    }
    (
        Layout {
            size: offset.next_multiple_of(align),
            align,
        },
        offsets,
    )
}

fn message_wire(defs: &mut impl Defs, message: &Message) -> MessageWire {
    let count = message.fields.len();
    let mut offsets = vec![0; count];
    let mut offset = 0usize;
    let mut align = 1usize;

    let mut fixed = Vec::new();
    let mut variable = Vec::new();
    for (index, field) in message.fields.iter().enumerate() {
        if is_fixed(defs, &field.ty) {
            fixed.push((index, layout(defs, &field.ty)));
        } else {
            variable.push(index);
        }
    }

    fixed.sort_by_key(|&(index, layout)| (Reverse(layout.align), index));
    for (index, layout) in fixed {
        offset = offset.next_multiple_of(layout.align);
        offsets[index] = offset;
        offset += layout.size;
        align = align.max(layout.align);
    }

    let mut presence = vec![None; count];
    let mut bits: u32 = 0;
    for (index, field) in message.fields.iter().enumerate() {
        if field.optional {
            presence[index] = Some(bits);
            bits += 1;
        }
    }
    let presence_offset = offset;
    let presence_bytes = (bits as usize).div_ceil(8);
    offset += presence_bytes;

    if !variable.is_empty() {
        offset = offset.next_multiple_of(2);
        align = align.max(2);
    }
    for index in variable {
        offsets[index] = offset;
        offset += 2;
        align = align.max(payload_align(defs, &message.fields[index].ty));
    }

    MessageWire {
        align,
        fixed_size: offset,
        presence_offset,
        presence_bytes,
        fields: offsets
            .into_iter()
            .zip(presence)
            .map(|(offset, presence)| FieldWire { offset, presence })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    const SAMPLE1: &str = include_str!("../../tests/data/sample1.uju");
    const SAMPLE2: &str = include_str!("../../tests/data/sample2.uju");

    fn wired(sources: &[&str]) -> (Schema, Wire) {
        let schema =
            compile(sources).unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"));
        let wire = Wire::new(&schema);
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

    fn message_field(schema: &Schema, wire: &Wire, message: &str, field: &str) -> FieldWire {
        let id = id(schema, message);
        let DefKind::Message(item) = &schema.def(id).kind else {
            panic!("`{message}` is not a message");
        };
        let DefWire::Message(item_wire) = wire.def(id) else {
            unreachable!();
        };
        let index = item
            .fields
            .iter()
            .position(|f| f.name == field)
            .unwrap_or_else(|| panic!("no field named `{field}`"));
        item_wire.fields[index]
    }

    #[test]
    fn structs_pack_by_alignment() {
        let (schema, wire) = wired(&["namespace a;\nstruct S { a: u8, b: u32, c: u16 }\n"]);
        let DefWire::Struct(s) = wire.def(id(&schema, "S")) else {
            panic!("expected a struct");
        };
        assert_eq!(s.offsets, [6, 0, 4]);
        assert_eq!(s.layout, Layout { size: 8, align: 4 });
    }

    #[test]
    fn struct_sizes_are_padded_to_their_alignment() {
        let (schema, wire) = wired(&["namespace a;\nstruct S { a: u32, b: u8 }\n"]);
        let DefWire::Struct(s) = wire.def(id(&schema, "S")) else {
            panic!("expected a struct");
        };
        assert_eq!(s.layout, Layout { size: 8, align: 4 });

        let (schema, wire) = wired(&[SAMPLE1, SAMPLE2]);
        let DefWire::Struct(empty) = wire.def(id(&schema, "EmptyStruct")) else {
            panic!("expected a struct");
        };
        assert_eq!(empty.layout, Layout { size: 0, align: 1 });
        assert_eq!(
            wire.layout(&Type::Named(id(&schema, "Position"))),
            Layout { size: 8, align: 4 }
        );
    }

    #[test]
    fn scalars_get_natural_layouts() {
        let (schema, wire) = wired(&[SAMPLE1]);
        let DefWire::Message(scalars) = wire.def(id(&schema, "Scalars")) else {
            panic!("expected a message");
        };
        assert_eq!(scalars.fixed_size, 95);
        assert_eq!(scalars.align, 8);
        assert_eq!(scalars.presence_bytes, 0);

        let offset = |name| message_field(&schema, &wire, "Scalars", name).offset;
        assert_eq!(offset("uint64"), 0);
        assert_eq!(offset("et"), 40);
        assert_eq!(offset("uint32"), 48);
        assert_eq!(offset("uet"), 60);
        assert_eq!(offset("uint16"), 72);
        assert_eq!(offset("uint8"), 76);
        assert_eq!(offset("id"), 79);
    }

    #[test]
    fn variable_fields_get_slots_and_presence_bits() {
        let (schema, wire) = wired(&[SAMPLE1]);
        let DefWire::Message(non_scalars) = wire.def(id(&schema, "NonScalars")) else {
            panic!("expected a message");
        };
        assert_eq!(non_scalars.presence_offset, 16);
        assert_eq!(non_scalars.presence_bytes, 1);
        assert_eq!(non_scalars.fixed_size, 44);
        assert_eq!(non_scalars.align, 4);

        let field = |name| message_field(&schema, &wire, "NonScalars", name);
        assert_eq!(field("ar").offset, 0, "the array is the only fixed field");
        assert_eq!(field("v").offset, 18, "slots start after the presence byte");
        assert_eq!(field("vo").offset, 32);
        assert_eq!(field("vo").presence, Some(0));
        assert_eq!(field("b").offset, 42);
        assert_eq!(field("b").presence, None);
    }

    #[test]
    fn optional_fixed_fields_reserve_inline_space() {
        let (schema, wire) = wired(&["namespace a;\nmessage M { a: i32?, b: u8 }\n"]);
        let DefWire::Message(m) = wire.def(id(&schema, "M")) else {
            panic!("expected a message");
        };
        assert_eq!(m.fixed_size, 6);
        assert_eq!(m.presence_offset, 5);
        assert_eq!(m.presence_bytes, 1);
        assert_eq!(message_field(&schema, &wire, "M", "a").offset, 0);
        assert_eq!(message_field(&schema, &wire, "M", "a").presence, Some(0));
        assert_eq!(message_field(&schema, &wire, "M", "b").presence, None);
    }

    #[test]
    fn payload_alignments() {
        let (schema, wire) = wired(&[SAMPLE1]);
        let position = Type::Named(id(&schema, "Position"));

        assert_eq!(wire.payload_align(&Type::Vec(Box::new(Type::U64))), 8);
        assert_eq!(wire.payload_align(&Type::String), 1);
        assert_eq!(
            wire.payload_align(&Type::Vec(Box::new(position.clone()))),
            4
        );
        assert_eq!(
            wire.payload_align(&Type::Vec(Box::new(Type::Vec(Box::new(Type::I32))))),
            4,
            "blocks of `vec<i32>` start with a u16 count padded to the item alignment"
        );
        assert_eq!(
            wire.payload_align(&Type::Vec(Box::new(Type::String))),
            2,
            "blocks of `string` are only aligned for their u16 length"
        );
        assert_eq!(
            wire.payload_align(&Type::Map(Box::new(Type::U8), Box::new(Type::U64))),
            8
        );
        assert_eq!(
            wire.payload_align(&Type::Map(
                Box::new(Type::U8),
                Box::new(Type::Vec(Box::new(position)))
            )),
            4
        );
    }

    #[test]
    fn map_entries_pack_like_structs() {
        let (_, wire) = wired(&[SAMPLE1]);
        let entry = wire.entry_layout(&Type::U8, &Type::U64);
        assert_eq!(entry.layout, Layout { size: 16, align: 8 });
        assert_eq!(entry.value_offset, 0);
        assert_eq!(entry.key_offset, 8);

        let tie = wire.entry_layout(&Type::U32, &Type::I32);
        assert_eq!(tie.key_offset, 0, "the key wins alignment ties");
        assert_eq!(tie.value_offset, 4);
    }

    #[test]
    fn union_payloads_align_to_their_largest_member() {
        let (schema, wire) = wired(&[SAMPLE1]);
        let DefWire::Union(some_union) = wire.def(id(&schema, "SomeUnion")) else {
            panic!("expected a union");
        };
        assert_eq!(some_union.payload_align, 4);
    }

    #[test]
    fn recursive_unions_reach_a_fixpoint() {
        let (schema, wire) = wired(&["namespace a;\nunion A { b: B, x: i64 }\nunion B { a: A }\n"]);
        let DefWire::Union(a) = wire.def(id(&schema, "A")) else {
            panic!("expected a union");
        };
        let DefWire::Union(b) = wire.def(id(&schema, "B")) else {
            panic!("expected a union");
        };
        assert_eq!(a.payload_align, 8);
        assert_eq!(b.payload_align, 8);
    }

    #[test]
    fn samples_lay_out() {
        let (schema, wire) = wired(&[SAMPLE1, SAMPLE2]);
        assert_eq!(wire.defs.len(), schema.defs.len());
        let DefWire::Message(external) = wire.def(id(&schema, "ExternalTypes")) else {
            panic!("expected a message");
        };
        assert_eq!(external.fixed_size, 16, "two 8-byte structs inline");
    }
}
