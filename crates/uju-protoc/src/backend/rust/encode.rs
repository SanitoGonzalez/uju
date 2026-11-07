//! Emits owned types' encoders: their size, and how they write.

use super::*;

impl Generator<'_> {
    /// The body of `encoded_size`, ending in its value: `extra` bytes, as a
    /// union's tag, then the object.
    pub(super) fn encoded_size(&self, out: &mut Out, object: &Object, extra: u32) {
        let layout = object.layout;
        if let Size::Fixed(size) = layout.size {
            out.line(format!("{}", extra + size));
            return;
        }
        out.line(format!("let mut size = {};", extra + layout.head));
        for (i, (field, slot)) in object.fields.iter().zip(&layout.fields).enumerate() {
            match slot {
                Slot::Optional { .. } => {
                    let size = self.fixed_size(&field.ty);
                    out.line(format!("if {}.is_some() {{", object.receiver(i)));
                    out.line(format!("size += {size};"));
                    out.line("}");
                }
                Slot::Var { presence, .. } => {
                    let size = self.size(&field.ty);
                    // Counted items take only their count's bytes.
                    if presence.is_none() && !size.contains('v') {
                        out.line(format!("size += {size};"));
                        continue;
                    }
                    if presence.is_some() {
                        out.line(format!("if let Some(v) = {} {{", object.reference(i)));
                    } else {
                        out.line("{");
                        out.line(format!("let v = {};", object.reference(i)));
                    }
                    out.line(format!("size += {size};"));
                    out.line("}");
                }
                Slot::Bit(_) | Slot::OptionalBit { .. } | Slot::At(_) => {}
            }
        }
        out.line("size");
    }

    /// The `impl __w::Encode` of a struct or message, or of a union, whose
    /// `write` is `body`.
    fn encode_impl(&self, out: &mut Out, name: &str, writer: &str, body: impl FnOnce(&mut Out)) {
        out.line(format!("impl __w::Encode for {name} {{"));
        out.line(format!(
            "const MAX_ENCODED_SIZE: {OPTION}<usize> = {name}::MAX_ENCODED_SIZE;"
        ));
        out.line("");
        out.line("fn encoded_size(&self) -> usize {");
        out.line(format!("{name}::encoded_size(self)"));
        out.line("}");
        out.line("");
        out.line(format!(
            "fn encode(&self, buf: &mut [u8]) -> {RESULT}<usize, __w::EncodeError> {{"
        ));
        out.line(format!("{name}::encode(self, buf)"));
        out.line("}");
        out.line("");
        out.line(format!(
            "fn write(&self, {writer}: &mut __w::en::Writer<'_>) -> {RESULT}<(), __w::EncodeError> {{"
        ));
        body(out);
        out.line("Ok(())");
        out.line("}");
        out.line("}");
    }

    pub(super) fn encoder(&self, out: &mut Out, object: &Object, name: &str) {
        // An object without fields writes nothing.
        let writer = if object.fields.is_empty() { "_w" } else { "w" };
        self.encode_impl(out, name, writer, |out| self.write_object(out, object));
    }

    /// Writes an object's fields through `w`, which starts where the object
    /// does.
    fn write_object(&self, out: &mut Out, object: &Object) {
        let layout = object.layout;
        let vars = layout.vars as usize;
        let width = layout.boundary.bytes() as usize;
        if vars > 1 {
            out.line("let start = w.pos();");
        }

        if layout.bitmap > 0 {
            out.line(format!("let mut bits = [0u8; {}];", layout.bitmap));
            for (i, (field, slot)) in object.fields.iter().zip(&layout.fields).enumerate() {
                // A `type` over `bool` is a bit too, a `.0` away per layer.
                let inner = ".0".repeat(self.newtypes(&field.ty, object.module).len());
                match *slot {
                    Slot::Bit(bit) => {
                        out.line(format!("if {} {{", object.value(i, &inner)));
                        out.line(set_bit(bit));
                        out.line("}");
                    }
                    Slot::OptionalBit { presence, value } => {
                        out.line(format!("if let Some(v) = {} {{", object.value(i, "")));
                        out.line(set_bit(presence));
                        out.line(format!("if v{inner} {{"));
                        out.line(set_bit(value));
                        out.line("}");
                        out.line("}");
                    }
                    Slot::Optional { presence }
                    | Slot::Var {
                        presence: Some(presence),
                        ..
                    } => {
                        out.line(format!("if {}.is_some() {{", object.receiver(i)));
                        out.line(set_bit(presence));
                        out.line("}");
                    }
                    Slot::At(_) | Slot::Var { presence: None, .. } => {}
                }
            }
            out.line("w.put(&bits)?;");
        }

        for (i, (field, slot)) in object.fields.iter().zip(&layout.fields).enumerate() {
            if let Slot::At(_) = slot {
                let label = object.field(field);
                out.line("{");
                out.line(format!("let v = {};", object.reference(i)));
                self.write(out, &field.ty, "v", &label);
                out.line("}");
            }
        }
        if vars > 1 {
            out.line(format!("let bounds = w.zeros({})?;", (vars - 1) * width));
        }
        for (i, (field, slot)) in object.fields.iter().zip(&layout.fields).enumerate() {
            if let Slot::Optional { .. } = slot {
                let label = object.field(field);
                out.line(format!("if let Some(v) = {} {{", object.reference(i)));
                self.write(out, &field.ty, "v", &label);
                out.line("}");
            }
        }
        for (i, (field, slot)) in object.fields.iter().zip(&layout.fields).enumerate() {
            let Slot::Var { slot, presence } = *slot else {
                continue;
            };
            let label = object.field(field);
            if presence.is_some() {
                out.line(format!("if let Some(v) = {} {{", object.reference(i)));
            } else {
                out.line("{");
                out.line(format!("let v = {};", object.reference(i)));
            }
            self.write(out, &field.ty, "v", &label);
            out.line("}");
            let slot = slot as usize;
            if slot + 1 < vars {
                let at = if slot == 0 {
                    "bounds".to_string()
                } else {
                    format!("bounds + {}", slot * width)
                };
                out.line(format!("w.end({at}, start, {width})?;"));
            }
        }
    }

    /// The `encoded_size` of union `id`, matching on `self`.
    pub(super) fn union_size(&self, out: &mut Out, id: TypeId, item: &Union, module: &[String]) {
        let layout = self.wire.union(id);
        let tag = layout.tag.bytes();
        out.line("match self {");
        for (index, variant) in item.variants.iter().enumerate() {
            let name = ident(&variant.name);
            match &variant.data {
                VariantData::Unit => out.line(format!("Self::{name} => {tag},")),
                VariantData::Type(ty) => match self.wire.size(ty) {
                    Size::Fixed(size) => out.line(format!("Self::{name}(_) => {},", tag + size)),
                    Size::Var { .. } => {
                        let size = self.size(ty);
                        let v = if size.contains('v') { "v" } else { "_" };
                        out.line(format!("Self::{name}({v}) => {tag} + {size},"));
                    }
                },
                VariantData::Fields(fields) => {
                    let object = self.variant_object(id, index, fields, module);
                    if let Size::Fixed(size) = object.layout.size {
                        out.line(format!("Self::{name} {{ .. }} => {},", tag + size));
                        continue;
                    }
                    out.line(format!("{} => {{", variant_pattern(&name, fields)));
                    self.encoded_size(out, &object, tag);
                    out.line("}");
                }
            }
        }
        out.line("}");
    }

    /// The `impl __w::Encode` of union `id`: its tag, then the variant's
    /// data.
    pub(super) fn union_encoder(&self, out: &mut Out, id: TypeId, item: &Union, module: &[String]) {
        let layout = self.wire.union(id);
        let name = &self.names[id.0];
        let label = self.schema.path(id).join(".");
        self.encode_impl(out, name, "w", |out| {
            out.line("match self {");
            for (index, variant) in item.variants.iter().enumerate() {
                let tag = match layout.tag {
                    Width::U8 => format!("w.put(&[{}])?;", UnionLayout::tag_of(index)),
                    _ => format!("w.put(&{}u16.to_le_bytes())?;", UnionLayout::tag_of(index)),
                };
                let name = ident(&variant.name);
                match &variant.data {
                    VariantData::Unit => {
                        out.line(format!("Self::{name} => {},", tag.trim_end_matches(';')));
                    }
                    VariantData::Type(ty) => {
                        out.line(format!("Self::{name}(v) => {{"));
                        out.line(tag);
                        self.write(out, ty, "v", &format!("{label}.{}", variant.name));
                        out.line("}");
                    }
                    VariantData::Fields(fields) => {
                        let object = self.variant_object(id, index, fields, module);
                        out.line(format!("{} => {{", variant_pattern(&name, fields)));
                        out.line(tag);
                        self.write_object(out, &object);
                        out.line("}");
                    }
                }
            }
            out.line("}");
        });
    }

    /// Writes `v`, a `&T` for the owned type of `ty`, through `w`.
    pub(super) fn write(&self, out: &mut Out, ty: &Type, v: &str, raw: &str) {
        let label = format!("{raw:?}");
        if let Some(int) = IntInfo::of(ty) {
            match int.repr {
                Int::Native(_) => {
                    if int.constrained() {
                        out.line(format!(
                            "__w::en::range(*{v}, {}, {}, {label})?;",
                            int.lit(int.lo),
                            int.lit(int.hi)
                        ));
                    }
                    out.line(format!("w.put(&{v}.to_le_bytes())?;"));
                }
                Int::Biased { width, .. } => {
                    out.line(format!(
                        "__w::en::range(*{v}, {}, {}, {label})?;",
                        int.lit(int.lo),
                        int.lit(int.hi)
                    ));
                    out.line(format!(
                        "w.put(&((*{v}).wrapping_sub({}) as {}).to_le_bytes())?;",
                        int.lit(int.lo),
                        unsigned(width)
                    ));
                }
            }
            return;
        }
        match &ty.kind {
            TypeKind::F32 | TypeKind::F64 => {
                if let Some(args) = float_args(ty) {
                    out.line(format!("__w::en::float(*{v}, {args}, {label})?;"));
                }
                out.line(format!("w.put(&{v}.to_le_bytes())?;"));
            }
            TypeKind::Bool => out.line(format!("w.put(&[*{v} as u8])?;")),
            TypeKind::Entity => {
                out.line(format!("w.put(&{v}.to_bits().to_le_bytes())?;"));
            }
            TypeKind::String => {
                if let Some(args) = length_args(&ty.constraints) {
                    out.line(format!("__w::en::length({v}.len(), {args}, {label})?;"));
                }
                out.line(format!("w.put({v}.as_bytes())?;"));
            }
            TypeKind::Bytes => {
                // A fixed length is the array's own.
                if ty.constraints.fixed_length().is_none()
                    && let Some(args) = length_args(&ty.constraints)
                {
                    out.line(format!("__w::en::length({v}.len(), {args}, {label})?;"));
                }
                out.line(format!("w.put({v})?;"));
            }
            TypeKind::Array(item, _) => {
                if is_raw_byte(item) {
                    out.line(format!("w.put({v})?;"));
                } else {
                    out.line(format!("for v in {v}.iter() {{"));
                    self.write(out, item, "v", raw);
                    out.line("}");
                }
            }
            TypeKind::Vec(item) | TypeKind::Set(item) => {
                if let Some(args) = length_args(&ty.constraints) {
                    out.line(format!("__w::en::length({v}.len(), {args}, {label})?;"));
                }
                match self.wire.sequence(ty).expect("a sequence") {
                    Sequence::Packed { .. }
                        if matches!(ty.kind, TypeKind::Vec(_)) && is_raw_byte(item) =>
                    {
                        out.line(format!("w.put({v})?;"));
                    }
                    Sequence::Packed { .. } => {
                        out.line(format!("for v in {v}.iter() {{"));
                        self.write(out, item, "v", raw);
                        out.line("}");
                    }
                    Sequence::Counted { count } => {
                        out.line(format!("w.len({}, {v}.len())?;", count.bytes()));
                    }
                    Sequence::Prefixed { count, prefix } => {
                        out.line(format!(
                            "__w::en::prefixed(w, {v}.iter(), {}, {}, |w, v| {{",
                            count.bytes(),
                            prefix.bytes()
                        ));
                        self.write(out, item, "v", raw);
                        out.line("Ok(())");
                        out.line("})?;");
                    }
                }
            }
            TypeKind::Map(key, value) => {
                if let Some(args) = length_args(&ty.constraints) {
                    out.line(format!("__w::en::length({v}.len(), {args}, {label})?;"));
                }
                match self.wire.sequence(ty).expect("a sequence") {
                    Sequence::Packed { .. } => {
                        out.line(format!("for (k, v) in {v}.iter() {{"));
                        self.write(out, key, "k", raw);
                        self.write(out, value, "v", raw);
                        out.line("}");
                    }
                    Sequence::Counted { count } => {
                        out.line(format!("w.len({}, {v}.len())?;", count.bytes()));
                    }
                    Sequence::Prefixed { count, prefix } => {
                        out.line(format!(
                            "__w::en::prefixed(w, {v}.iter(), {}, {}, |w, (k, v)| {{",
                            count.bytes(),
                            prefix.bytes()
                        ));
                        self.write(out, key, "k", raw);
                        self.write(out, value, "v", raw);
                        out.line("Ok(())");
                        out.line("})?;");
                    }
                }
            }
            TypeKind::Named(id) => match &self.schema.def(*id).kind {
                DefKind::Enum(_) => {
                    out.line(format!("w.put(__w::Enum::to_bytes(*{v}).as_ref())?;"));
                }
                DefKind::Type(item) => {
                    out.line("{");
                    out.line(format!("let v = &{v}.0;"));
                    self.write(out, &item.target, "v", raw);
                    out.line("}");
                }
                _ => out.line(format!("__w::Encode::write({v}, w)?;")),
            },
            _ => unreachable!("integers are handled above"),
        }
    }

    /// The encoded size of `v`, a `&T` for the owned type of `ty`.
    pub(super) fn size(&self, ty: &Type) -> String {
        if let Size::Fixed(size) = self.wire.size(ty) {
            return size.to_string();
        }
        match &ty.kind {
            TypeKind::String | TypeKind::Bytes => "v.len()".to_string(),
            TypeKind::Vec(item) | TypeKind::Set(item) => {
                match self.wire.sequence(ty).expect("a sequence") {
                    Sequence::Packed { item: 1 } => "v.len()".to_string(),
                    Sequence::Packed { item } => format!("v.len() * {item}"),
                    Sequence::Counted { count } => count.bytes().to_string(),
                    Sequence::Prefixed { count, prefix } => format!(
                        "__w::en::prefixed_size(v.iter(), {}, {}, |v| {})",
                        count.bytes(),
                        prefix.bytes(),
                        self.size(item)
                    ),
                }
            }
            TypeKind::Map(key, value) => match self.wire.sequence(ty).expect("a sequence") {
                Sequence::Packed { item: 1 } => "v.len()".to_string(),
                Sequence::Packed { item } => format!("v.len() * {item}"),
                Sequence::Counted { count } => count.bytes().to_string(),
                Sequence::Prefixed { count, prefix } => {
                    let key = self.fixed_size(key);
                    let value = self.size(value);
                    let entry = if key == 0 {
                        value
                    } else {
                        format!("{key} + {value}")
                    };
                    format!(
                        "__w::en::prefixed_size(v.iter(), {}, {}, |(_, v)| {entry})",
                        count.bytes(),
                        prefix.bytes()
                    )
                }
            },
            TypeKind::Named(_) => "__w::Encode::encoded_size(v)".to_string(),
            _ => unreachable!("only strings, bytes, collections, structs, and unions vary in size"),
        }
    }
}
