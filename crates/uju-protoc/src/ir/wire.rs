//! Wire layout of a lowered [`Schema`]: where each value sits in an encoded
//! message, and how wide each integer, count, length, and offset is. Every
//! backend reads its layout from here, so all targets agree byte for byte.
//! `docs/protocol/wire-format.md` specifies the format itself.

mod fingerprint;
mod ids;
mod layout;

use crate::ir::{Role, Schema, Type, TypeId};

use fingerprint::{Scope, canonical, client_listed, fnv1a};
use ids::Ids;
pub use ids::Space;
pub use layout::{
    Int, Layout, PatchLayout, PatchSlot, PatchValue, Sequence, Size, Slot, UnionLayout, Width,
};
use layout::{Layouts, sequence, type_size};

/// The largest encoded message, in bytes. Every length, count, and offset
/// therefore fits in a `u16`.
pub const MAX_SIZE: u32 = u16::MAX as u32;

/// The most variants a union holds, so its tag fits in a `u16`.
pub const MAX_VARIANTS: usize = u16::MAX as usize;

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
    layouts: Layouts,
    ids: Ids,
    fingerprints: Fingerprints,
    /// Indexed by [`TypeId`]: whether the client fingerprint lists it.
    client: Vec<bool>,
}

impl Wire {
    pub fn new(schema: &Schema) -> Result<Wire, Vec<Error>> {
        let mut errors = Vec::new();
        let layouts = Layouts::new(schema, &mut errors);
        let ids = Ids::new(schema, &mut errors);
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Wire {
            layouts,
            ids,
            fingerprints: Fingerprints {
                client: fnv1a(canonical(schema, Scope::Client).as_bytes()),
                internal: fnv1a(canonical(schema, Scope::Internal).as_bytes()),
            },
            client: client_listed(schema),
        })
    }

    pub fn size(&self, ty: &Type) -> Size {
        type_size(ty, &mut |id| self.layouts.sizes[id.0])
    }

    /// How `ty` lays out its items, when it is a `vec`, `set`, or `map`.
    pub fn sequence(&self, ty: &Type) -> Option<Sequence> {
        sequence(ty, &mut |id| self.layouts.sizes[id.0]).map(|(sequence, _)| sequence)
    }

    /// The layout of a struct, message, hub kind, or component.
    ///
    /// # Panics
    ///
    /// When `id` is not a struct, message, hub kind, or component.
    pub fn layout(&self, id: TypeId) -> &Layout {
        self.layouts.layouts[id.0]
            .as_ref()
            .expect("only structs, messages, hub kinds, and components have a layout")
    }

    /// The layout of a union.
    ///
    /// # Panics
    ///
    /// When `id` is not a union.
    pub fn union(&self, id: TypeId) -> &UnionLayout {
        self.layouts.unions[id.0].as_ref().expect("not a union")
    }

    /// The layout of component `id`'s patch, `None` for anything but a
    /// `@replicate(patch)` component.
    pub fn patch(&self, id: TypeId) -> Option<&PatchLayout> {
        self.layouts.patches[id.0].as_ref()
    }

    /// Message `id`'s id on each of its links, in the order of
    /// [`Message::links`]. Empty for anything but a message.
    ///
    /// [`Message::links`]: crate::ir::Message::links
    pub fn ids(&self, id: TypeId) -> &[u16] {
        &self.ids.messages[id.0]
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
        &self.ids.spaces
    }

    /// The messages linking `from` to `to`, in id order from 1.
    pub fn space(&self, from: Role, to: Role) -> &[TypeId] {
        self.ids
            .spaces
            .iter()
            .find(|space| space.from == from && space.to == to)
            .map_or(&[], |space| &space.messages)
    }

    /// The id of hub kind `id`, `None` for any other declaration.
    pub fn hub_id(&self, id: TypeId) -> Option<u16> {
        self.ids.hubs.id(id)
    }

    /// Every hub kind, in id order from 1.
    pub fn hubs(&self) -> &[TypeId] {
        &self.ids.hubs.order
    }

    /// The id of component or tag `id`, `None` for any other declaration.
    pub fn component_id(&self, id: TypeId) -> Option<u16> {
        self.ids.components.id(id)
    }

    /// Every component and tag, in id order from 1: those clients see,
    /// by qualified name, then those with `visibility=none`, by qualified
    /// name.
    pub fn components(&self) -> &[TypeId] {
        &self.ids.components.order
    }

    /// The components and tags clients see, which take ids
    /// `1..=client_components().len()`.
    pub fn client_components(&self) -> &[TypeId] {
        &self.ids.components.order[..self.ids.client_components]
    }

    /// The id of audience `id`, `None` for any other declaration.
    pub fn audience_id(&self, id: TypeId) -> Option<u16> {
        self.ids.audiences.id(id)
    }

    /// Every audience, in id order from 1.
    pub fn audiences(&self) -> &[TypeId] {
        &self.ids.audiences.order
    }

    /// Hashes of what peers must agree on, names included.
    pub fn fingerprints(&self) -> Fingerprints {
        self.fingerprints
    }

    /// Whether the client fingerprint lists declaration `id`, so code for
    /// clients holds it. A hub kind is listed by its key and name only,
    /// and its fields are not.
    pub fn client(&self, id: TypeId) -> bool {
        self.client[id.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;
    use crate::ir::{DefKind, Field};

    pub(super) const EXAMPLE1: &str = include_str!("../../tests/data/example1.uju");

    pub(super) const EXAMPLE2: &str = include_str!("../../tests/data/example2.uju");

    pub(super) fn compiled(sources: &[&str]) -> (Schema, Wire) {
        let schema = compile(sources)
            .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
            .schema;
        let wire = Wire::new(&schema).expect("lowering checks the layout");
        (schema, wire)
    }

    pub(super) fn id(schema: &Schema, name: &str) -> TypeId {
        TypeId(
            schema
                .defs
                .iter()
                .position(|def| def.name == name)
                .unwrap_or_else(|| panic!("no declaration named `{name}`")),
        )
    }

    pub(super) fn fields(schema: &Schema, id: TypeId) -> &[Field] {
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

    pub(super) fn field<'a>(schema: &'a Schema, id: TypeId, name: &str) -> &'a Type {
        &fields(schema, id)
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("no field named `{name}`"))
            .ty
    }

    /// The layout of the only message of `M`, after a `namespace a` header.
    pub(super) fn message(body: &str) -> (Schema, Wire, TypeId) {
        let (schema, wire) = compiled(&[&format!("namespace a\n{body}\n")]);
        let id = id(&schema, "M");
        (schema, wire, id)
    }
}
