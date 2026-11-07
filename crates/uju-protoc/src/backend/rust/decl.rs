//! Emits each kind of declaration, and the ids and dispatch enums in the
//! root.

use super::*;

impl Generator<'_> {
    pub(super) fn constant(&self, out: &mut Out, id: ConstId) {
        let item = self.schema.constant(id);
        let (ty, value) = match (&item.ty, &item.value) {
            (_, ConstValue::Int(value)) => (self.schema.kind_name(&item.ty), value.to_string()),
            (TypeKind::F32, ConstValue::Float(value)) => {
                ("f32".into(), format!("{:?}", *value as f32))
            }
            (_, ConstValue::Float(value)) => ("f64".into(), format!("{value:?}")),
            (_, ConstValue::Bool(value)) => ("bool".into(), value.to_string()),
            (_, ConstValue::String(value)) => ("&str".into(), string_literal(value)),
        };
        out.line(format!(
            "pub const {}: {ty} = {value};",
            self.const_names[id.0]
        ));
    }

    pub(super) fn enumeration(&self, out: &mut Out, id: TypeId, item: &crate::ir::Enum) {
        let name = &self.names[id.0];
        let repr = item.repr.name();
        out.line(format!("#[repr({repr})]"));
        out.line("#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]");
        out.line(format!("pub enum {name} {{"));
        for (index, variant) in item.variants.iter().enumerate() {
            if index == 0 {
                out.line("#[default]");
            }
            out.line(format!("{} = {},", ident(&variant.name), variant.value));
        }
        out.line("}");
        out.line("");
        out.line(format!("unsafe impl __w::Enum for {name} {{"));
        out.line(format!("type Bytes = [u8; {}];", item.repr.size()));
        out.line("");
        out.line(format!(
            "fn from_bytes(bytes: Self::Bytes) -> {OPTION}<Self> {{"
        ));
        out.line(format!("match {repr}::from_le_bytes(bytes) {{"));
        for variant in &item.variants {
            out.line(format!(
                "{} => Some({name}::{}),",
                variant.value,
                ident(&variant.name)
            ));
        }
        out.line("_ => None,");
        out.line("}");
        out.line("}");
        out.line("");
        out.line("fn to_bytes(self) -> Self::Bytes {");
        out.line(format!("(self as {repr}).to_le_bytes()"));
        out.line("}");
        out.line("}");
    }

    pub(super) fn object(&self, out: &mut Out, id: TypeId, fields: &[Field], module: &[String]) {
        let layout = self.wire.layout(id);
        let name = &self.names[id.0];
        let traits = self.traits[id.0];
        let mut derives = vec!["Debug", "Clone"];
        if traits.copy {
            derives.push("Copy");
        }
        derives.push("PartialEq");
        if traits.eq {
            derives.push("Eq");
        }
        if traits.hash {
            derives.push("Hash");
        }
        if traits.default {
            derives.push("Default");
        }
        out.line(format!("#[derive({})]", derives.join(", ")));
        if fields.is_empty() {
            out.line(format!("pub struct {name} {{}}"));
        } else {
            out.line(format!("pub struct {name} {{"));
            for field in fields {
                out.line(format!(
                    "pub {}: {},",
                    ident(&field.name),
                    self.field_owned(field, module)
                ));
            }
            out.line("}");
        }

        let object = Object {
            name: self.schema.path(id).join("."),
            layout,
            fields,
            module,
        };
        out.line("");
        out.line(format!("impl {name} {{"));
        if self.encode[id.0] {
            let max = match layout.size.max() {
                Some(max) => format!("Some({max})"),
                None => "None".to_string(),
            };
            out.line(format!(
                "pub const MAX_ENCODED_SIZE: {OPTION}<usize> = {max};"
            ));
            out.line("");
            out.line("pub fn encoded_size(&self) -> usize {");
            self.encoded_size(out, &object);
            out.line("}");
            out.line("");
            out.line("/// Writes the encoding to the start of `buf`, which must hold at least");
            out.line("/// [`Self::encoded_size`] bytes, and returns its length.");
            out.line(format!(
                "pub fn encode(&self, buf: &mut [u8]) -> {RESULT}<usize, __w::EncodeError> {{"
            ));
            out.line(format!("__w::en::encode(self, buf, {:?})", object.name));
            out.line("}");
        }
        if self.decode[id.0] {
            if self.encode[id.0] {
                out.line("");
            }
            out.line("/// Validates `buf` as one encoding and copies it out.");
            out.line(format!(
                "pub fn decode(buf: &[u8]) -> {RESULT}<{name}, __w::DecodeError> {{"
            ));
            out.line(format!(
                "{name}View::decode(buf).map(|view| view.to_owned())"
            ));
            out.line("}");
        }
        out.line("}");

        if self.encode[id.0] {
            out.line("");
            self.encoder(out, &object, name);
        }
        if self.decode[id.0] {
            out.line("");
            self.view(out, &object, name);
        }
    }

    pub(super) fn message(&self, out: &mut Out, id: TypeId) {
        let name = &self.names[id.0];
        out.line("");
        out.line(format!("impl __w::Message for {name} {{"));
        out.line(format!(
            "const NAME: &'static str = {:?};",
            self.schema.qualified(id)
        ));
        out.line("}");
    }

    /// Every message's ids, in the root: one message added or removed
    /// renumbers those after it, whatever their namespace.
    pub(super) fn ids(&self, out: &mut Out) {
        for (way, messages, name) in [
            (Direction::In, self.wire.inbound(), "Inbound"),
            (Direction::Out, self.wire.outbound(), "Outbound"),
        ] {
            let constant = name.to_uppercase();
            for id in messages {
                let ty = self.path(*id, &[], &self.names[id.0]);
                out.line("");
                out.line(format!("impl {RUNTIME}::{name} for {ty} {{"));
                out.line(format!(
                    "const {constant}_ID: u16 = {};",
                    self.message_id(*id, way)
                ));
                out.line("}");
            }
        }
    }

    /// The id of message `id` among those travelling `way`.
    pub(super) fn message_id(&self, id: TypeId, way: Direction) -> u16 {
        let ids = self.wire.ids(id);
        match way {
            Direction::In => ids.inbound,
            _ => ids.outbound,
        }
        .expect("a message travelling `way`")
    }

    /// The enum of every message travelling `way`, decoded by id.
    pub(super) fn dispatch(&self, out: &mut Out, way: Direction) {
        let (name, messages, receiver) = match way {
            Direction::In => ("InboundMessage", self.wire.inbound(), "server receives"),
            _ => ("OutboundMessage", self.wire.outbound(), "client receives"),
        };
        let variants = self.variants(messages);
        out.line("");
        out.line(format!(
            "/// A message a {receiver}, decoded by its id and viewed in place."
        ));
        out.line("#[derive(Debug, Clone, Copy)]");
        out.line(format!("pub enum {name}<'a> {{"));
        for (variant, id) in variants.iter().zip(messages) {
            out.line(format!("{variant}({}),", self.root_view(*id)));
        }
        out.line("}");
        out.line("");
        out.line(format!("impl<'a> {name}<'a> {{"));
        out.line(format!(
            "pub fn decode(id: u16, buf: &'a [u8]) -> {RESULT}<Self, {RUNTIME}::DecodeError> {{"
        ));
        out.line("match id {");
        for (variant, id) in variants.iter().zip(messages) {
            let view = self.path(*id, &[], &format!("{}View", self.names[id.0]));
            out.line(format!(
                "{} => {view}::decode(buf).map(Self::{variant}),",
                self.message_id(*id, way)
            ));
        }
        out.line(format!("_ => Err({RUNTIME}::DecodeError::unknown(id)),"));
        out.line("}");
        out.line("}");
        out.line("");
        out.line("pub fn id(&self) -> u16 {");
        out.line("match self {");
        for (variant, id) in variants.iter().zip(messages) {
            out.line(format!(
                "Self::{variant}(_) => {},",
                self.message_id(*id, way)
            ));
        }
        out.line("}");
        out.line("}");
        out.line("}");
    }

    /// Dispatch variant names: each message's Rust name, or its qualified
    /// name in Pascal case where two namespaces share a name.
    pub(super) fn variants(&self, messages: &[TypeId]) -> Vec<String> {
        messages
            .iter()
            .map(|id| {
                let name = &self.names[id.0];
                let shared = messages
                    .iter()
                    .filter(|other| self.names[other.0] == *name)
                    .count()
                    > 1;
                if !shared {
                    return name.clone();
                }
                let namespace = self.schema.namespace(self.schema.def(*id).namespace);
                let mut variant: String = namespace.name.iter().map(|s| case::pascal(s)).collect();
                variant.push_str(name);
                variant
            })
            .collect()
    }

    /// A message's view type from the root module.
    pub(super) fn root_view(&self, id: TypeId) -> String {
        let view = self.path(id, &[], &format!("{}View", self.names[id.0]));
        if self.is_fixed_def(id) {
            format!("&'a {view}")
        } else {
            format!("{view}<'a>")
        }
    }
}
