//! Emits views, which read an encoding in place, and their validation.

use super::*;

impl Generator<'_> {
    /// The view of struct or message `name`, or with `variant`, the path of
    /// a union's variant, of that variant's fields. A variant's view is only
    /// reached through the union's, so it leaves out what decodes it alone.
    pub(super) fn view(&self, out: &mut Out, object: &Object, name: &str, variant: Option<&str>) {
        let layout = object.layout;
        let doc = variant.unwrap_or(name);
        let fixed = layout.size.fixed();
        let view = format!("{name}View");
        let (lt, bytes) = if fixed.is_some() {
            ("'_", "&self.0")
        } else {
            ("'a", "self.buf")
        };
        let (ty, generics) = match fixed {
            Some(_) => (view.clone(), String::new()),
            None => (format!("{view}<'a>"), "<'a>".to_string()),
        };

        match fixed {
            Some(size) => {
                out.line(format!(
                    "/// [`{doc}`], read in place; borrowed as `&{view}`."
                ));
                out.line("#[repr(transparent)]");
                out.line("#[derive(Clone, Copy)]");
                out.line(format!("pub struct {view}([u8; {size}]);"));
            }
            None => {
                out.line(format!("/// [`{doc}`], read in place."));
                out.line("#[derive(Clone, Copy)]");
                out.line(format!("pub struct {view}<'a> {{"));
                out.line("buf: &'a [u8],");
                out.line("}");
            }
        }

        out.line("");
        out.line(format!("impl{generics} {ty} {{"));
        if variant.is_none() {
            let (input, output) = match fixed {
                Some(_) => ("&[u8]", format!("&{view}")),
                None => ("&'a [u8]", "Self".to_string()),
            };
            let unchecked = match fixed {
                Some(_) => format!("unsafe {{ __w::cast::<{view}>(buf) }}"),
                None => format!("{view} {{ buf }}"),
            };
            let validated = match fixed {
                Some(_) => view.clone(),
                None => format!("{view}<'_>"),
            };
            self.decoders(out, &object.name, input, &output, &unchecked, &validated);
            out.line("");
        }
        let as_bytes = if fixed.is_some() { "&[u8]" } else { "&'a [u8]" };
        out.line(format!("pub fn as_bytes(&self) -> {as_bytes} {{"));
        out.line(bytes);
        out.line("}");
        if variant.is_none() {
            out.line("");
            out.line(format!("pub fn to_owned(&self) -> {name} {{"));
            out.line(self.owned_object(object, name, "self"));
            out.line("}");
        }

        let optionals = self.optionals(object);
        let start = match optionals.as_str() {
            "" => layout.head.to_string(),
            list => format!(
                "__w::offset(b, {}, &[{list}], {})",
                layout.head,
                object.optional_count()
            ),
        };
        let mut k = 0;
        for (field, slot) in object.fields.iter().zip(&layout.fields) {
            out.line("");
            let ty = self.view_ty(&field.ty, object.module, lt);
            let ty = if field.optional {
                format!("{OPTION}<{ty}>")
            } else {
                ty
            };
            out.line(format!(
                "pub fn {}(&self) -> {ty} {{",
                accessor(&field.name)
            ));
            out.line(format!(
                "let b: &{} [u8] = {bytes};",
                lt.trim_start_matches("'_")
            ));
            match *slot {
                Slot::Bit(bit) => out.line(self.bit(&field.ty, bit, object.module)),
                Slot::OptionalBit { presence, value } => {
                    out.line(format!("if __w::bit(b, {presence}) {{"));
                    out.line(format!(
                        "Some({})",
                        self.bit(&field.ty, value, object.module)
                    ));
                    out.line("} else {");
                    out.line("None");
                    out.line("}");
                }
                Slot::At(at) => {
                    let cell = self.cell(&field.ty, object.module);
                    out.line(format!("unsafe {{ __w::read::<{cell}>(b, {at}) }}"));
                }
                Slot::Optional { presence } => {
                    let cell = self.cell(&field.ty, object.module);
                    out.line(format!("if __w::bit(b, {presence}) {{"));
                    out.line(format!(
                        "let at = __w::offset(b, {}, &[{optionals}], {k});",
                        layout.head
                    ));
                    out.line(format!("Some(unsafe {{ __w::read::<{cell}>(b, at) }})"));
                    out.line("} else {");
                    out.line("None");
                    out.line("}");
                    k += 1;
                }
                Slot::Var { slot, presence } => {
                    let var = self.var_view(&field.ty, object.module, lt);
                    let read = [
                        format!(
                            "let range = __w::slot(b, {start}, {}, {}, {}, {slot});",
                            layout.boundaries,
                            layout.boundary.bytes(),
                            layout.vars
                        ),
                        format!("unsafe {{ <{var} as __w::Var<'a>>::read(&b[range]) }}"),
                    ];
                    match presence {
                        Some(presence) => {
                            out.line(format!("if __w::bit(b, {presence}) {{"));
                            out.line(read[0].clone());
                            out.line(format!("Some({})", read[1]));
                            out.line("} else {");
                            out.line("None");
                            out.line("}");
                        }
                        None => {
                            out.line(read[0].clone());
                            out.line(read[1].clone());
                        }
                    }
                }
            }
            out.line("}");
        }
        out.line("}");

        out.line("");
        out.line(format!(
            "impl __w::Validate for {} {{",
            if fixed.is_some() {
                view.clone()
            } else {
                format!("{view}<'_>")
            }
        ));
        out.line(format!(
            "fn validate_at(buf: &[u8], at: usize) -> {RESULT}<(), __w::DecodeError> {{"
        ));
        self.validate(out, object);
        out.line("Ok(())");
        out.line("}");
        out.line("}");

        out.line("");
        match fixed {
            Some(_) => {
                out.line(format!("unsafe impl __w::Cell for {view} {{"));
                out.line(format!("type Value<'a> = &'a {view};"));
                out.line("");
                out.line(format!("fn get(&self) -> &{view} {{"));
                out.line("self");
                out.line("}");
                out.line("}");
            }
            None => {
                out.line(format!("impl<'a> __w::Var<'a> for {view}<'a> {{"));
                out.line("unsafe fn read(bytes: &'a [u8]) -> Self {");
                out.line(format!("{view} {{ buf: bytes }}"));
                out.line("}");
                out.line("}");
            }
        }
        if variant.is_none() {
            let target = match fixed {
                Some(_) => format!("&'a {view}"),
                None => format!("{view}<'a>"),
            };
            out.line("");
            decode_impl(out, &target, &view);
        }

        out.line("");
        out.line(format!(
            "impl ::core::fmt::Debug for {} {{",
            if fixed.is_some() {
                view.clone()
            } else {
                format!("{view}<'_>")
            }
        ));
        out.line("fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {");
        out.line(format!("f.debug_struct({view:?})"));
        for field in object.fields {
            out.line(format!(
                ".field({:?}, &self.{}())",
                field.name,
                accessor(&field.name)
            ));
        }
        out.line(".finish()");
        out.line("}");
        out.line("}");
    }

    /// The body of `validate_at`: checks `buf`, exactly one encoding, that
    /// starts `at` bytes into the message.
    pub(super) fn validate(&self, out: &mut Out, object: &Object) {
        let layout = object.layout;
        let name = format!("{:?}", object.name);
        let fixed = layout.size.fixed().is_some();
        if fixed {
            out.line(format!(
                "__w::de::exact(buf, {}, at, {name})?;",
                layout.head
            ));
        } else {
            out.line(format!(
                "__w::de::at_least(buf, {}, at, {name})?;",
                layout.head
            ));
        }

        let bits = layout
            .fields
            .iter()
            .map(|slot| match slot {
                Slot::Bit(bit) | Slot::Optional { presence: bit } => bit + 1,
                Slot::OptionalBit { value, .. } => value + 1,
                Slot::Var {
                    presence: Some(bit),
                    ..
                } => bit + 1,
                Slot::At(_) | Slot::Var { presence: None, .. } => 0,
            })
            .max()
            .unwrap_or(0);
        if !bits.is_multiple_of(8) {
            let last = layout.bitmap - 1;
            let mask = 0xffu8 << (bits % 8);
            out.line(format!(
                "__w::de::unused(buf[{last}], 0x{mask:02x}, at + {last}, {name})?;"
            ));
        }
        for (field, slot) in object.fields.iter().zip(&layout.fields) {
            if let Slot::OptionalBit { presence, value } = slot {
                out.line(format!(
                    "__w::de::option_bool(buf, {presence}, {value}, at, {:?})?;",
                    object.field(field)
                ));
            }
        }
        for (field, slot) in object.fields.iter().zip(&layout.fields) {
            if let Slot::At(offset) = *slot {
                let check = self.check_fixed(&field.ty, &object.field(field), object.module);
                if !check.is_empty() {
                    let end = offset + self.fixed_size(&field.ty);
                    out.line("{");
                    out.line(format!(
                        "let (b, at) = (&buf[{offset}..{end}], at + {offset});"
                    ));
                    out.extend(check);
                    out.line("}");
                }
            }
        }
        if fixed {
            return;
        }

        let vars = layout.vars as usize;
        let mut slots = Vec::new();
        for (field, slot) in object.fields.iter().zip(&layout.fields) {
            if let Slot::Var { slot, presence } = *slot {
                let check = self.check_var(&field.ty, &object.field(field), object.module);
                slots.push((slot as usize, presence, check, object.field(field)));
            }
        }
        // `pos`, where the var section starts, is read unless only one slot
        // without checks follows.
        let read = vars != 1
            || slots
                .iter()
                .any(|(_, presence, check, _)| presence.is_some() || !check.is_empty());
        if object.optional_count() > 0 {
            out.line(format!("let mut pos = {};", layout.head));
            for (field, slot) in object.fields.iter().zip(&layout.fields) {
                let Slot::Optional { presence } = *slot else {
                    continue;
                };
                let label = object.field(field);
                let size = self.fixed_size(&field.ty);
                out.line(format!("if __w::bit(buf, {presence}) {{"));
                out.line(format!(
                    "__w::de::at_least(buf, pos + {size}, at, {label:?})?;"
                ));
                let check = self.check_fixed(&field.ty, &label, object.module);
                if !check.is_empty() {
                    out.line("{");
                    out.line(format!(
                        "let (b, at) = (&buf[pos..pos + {size}], at + pos);"
                    ));
                    out.extend(check);
                    out.line("}");
                }
                out.line(format!("pos += {size};"));
                out.line("}");
            }
            if !read {
                out.line("let _ = pos;");
            }
        } else if read {
            out.line(format!("let pos = {};", layout.head));
        }
        if vars == 0 {
            out.line(format!("__w::de::exact(buf, pos, at, {name})?;"));
        }
        if vars > 1 {
            out.line(format!(
                "let ends = __w::de::boundaries::<{}>(buf, {}, {}, pos, at, {name})?;",
                vars - 1,
                layout.boundaries,
                layout.boundary.bytes()
            ));
        }
        for (slot, presence, check, label) in slots {
            if presence.is_none() && check.is_empty() {
                continue;
            }
            let start = if slot == 0 {
                "pos".to_string()
            } else {
                format!("ends[{}]", slot - 1)
            };
            let end = if slot + 1 == vars {
                "buf.len()".to_string()
            } else {
                format!("ends[{slot}]")
            };
            out.line("{");
            out.line(format!(
                "let (b, at) = (&buf[{start}..{end}], at + {start});"
            ));
            match presence {
                Some(presence) => {
                    out.line(format!("if __w::bit(buf, {presence}) {{"));
                    out.extend(check);
                    out.line("} else {");
                    out.line(format!("__w::de::empty(b, at, {label:?})?;"));
                    out.line("}");
                }
                None => out.extend(check),
            }
            out.line("}");
        }
    }

    /// Checks `b`, the bytes of a fixed-size `ty` starting `at` bytes into
    /// the message, beyond their size; empty when any bytes will do.
    pub(super) fn check_fixed(&self, ty: &Type, raw: &str, module: &[String]) -> Vec<String> {
        let label = format!("{raw:?}");
        if let Some(int) = IntInfo::of(ty) {
            return match int.repr {
                Int::Native(_) if int.constrained() => vec![format!(
                    "__w::de::range(__w::de::plain::<{}>(b, 0), {}, {}, at, {label})?;",
                    int.cell(),
                    int.lit(int.lo),
                    int.lit(int.hi)
                )],
                Int::Biased { width, .. } if int.span() < width_max(width) => vec![format!(
                    "__w::de::range(__w::de::plain::<{}>(b, 0), 0, {}, at, {label})?;",
                    stored(width),
                    int.span()
                )],
                _ => Vec::new(),
            };
        }
        match &ty.kind {
            TypeKind::F32 | TypeKind::F64 => match float_args(ty) {
                Some(args) => vec![format!(
                    "__w::de::float(__w::de::plain::<{}>(b, 0), {args}, at, {label})?;",
                    self.cell(ty, module)
                )],
                None => Vec::new(),
            },
            TypeKind::Bool => vec![format!("__w::de::boolean(b[0], at, {label})?;")],
            TypeKind::Entity => vec![format!("__w::de::entity(b, at, {label})?;")],
            TypeKind::String => vec![format!("__w::de::utf8(b, at, {label})?;")],
            TypeKind::Array(item, len) => {
                let check = self.check_fixed(item, raw, module);
                if check.is_empty() {
                    return check;
                }
                let size = self.fixed_size(item);
                let mut lines = vec![
                    format!("for i in 0..{len} {{"),
                    format!("let (b, at) = (&b[i * {size}..(i + 1) * {size}], at + i * {size});"),
                ];
                lines.extend(check);
                lines.push("}".to_string());
                lines
            }
            TypeKind::Named(id) => match &self.schema.def(*id).kind {
                DefKind::Enum(_) => vec![format!(
                    "__w::de::enumeration::<{}>(b, at, {label})?;",
                    self.path(*id, module, &self.names[id.0])
                )],
                // The target's constraints hold wherever the `type` is used.
                DefKind::Type(item) => self.check_fixed(&item.target, raw, module),
                _ if self.is_checked(*id) => vec![format!(
                    "<{} as __w::Validate>::validate_at(b, at)?;",
                    self.view_path(*id, module)
                )],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Checks `b`, exactly the bytes of a variable-size `ty` starting `at`
    /// bytes into the message; empty when any bytes will do.
    pub(super) fn check_var(&self, ty: &Type, label: &str, module: &[String]) -> Vec<String> {
        let quoted = format!("{label:?}");
        let length = length_args(&ty.constraints);
        let mut lines = Vec::new();
        match &ty.kind {
            TypeKind::String | TypeKind::Bytes => {
                if ty.kind == TypeKind::String {
                    lines.push(format!("__w::de::utf8(b, at, {quoted})?;"));
                }
                if let Some(args) = length {
                    lines.push(format!("__w::de::length(b.len(), {args}, at, {quoted})?;"));
                }
            }
            TypeKind::Vec(_) | TypeKind::Set(_) | TypeKind::Map(..) => {
                let (key, item) = match &ty.kind {
                    TypeKind::Map(key, value) => (Some(&**key), &**value),
                    TypeKind::Vec(item) | TypeKind::Set(item) => (None, &**item),
                    _ => unreachable!(),
                };
                let key_size = key.map_or(0, |key| self.fixed_size(key));
                let (count, body) = match self.wire.sequence(ty).expect("a sequence") {
                    Sequence::Packed { item: size } => {
                        let mut body = Vec::new();
                        let fixed_item = |at: &str, ty: &Type, body: &mut Vec<String>| {
                            let check = self.check_fixed(ty, label, module);
                            if !check.is_empty() {
                                body.push("{".to_string());
                                body.push(at.to_string());
                                body.extend(check);
                                body.push("}".to_string());
                            }
                        };
                        if let Some(key) = key {
                            fixed_item(
                                &format!("let (b, at) = (&b[..{key_size}], at);"),
                                key,
                                &mut body,
                            );
                            fixed_item(
                                &format!("let (b, at) = (&b[{key_size}..], at + {key_size});"),
                                item,
                                &mut body,
                            );
                        } else {
                            body = self.check_fixed(item, label, module);
                        }
                        let mut looped = Vec::new();
                        if !body.is_empty() {
                            looped.push("for i in 0..n {".to_string());
                            looped.push(format!(
                                "let (b, at) = (&b[i * {size}..(i + 1) * {size}], at + i * {size});"
                            ));
                            looped.extend(body);
                            looped.push("}".to_string());
                        }
                        (format!("__w::de::packed(b, {size}, at, {quoted})?"), looped)
                    }
                    Sequence::Counted { count } => (
                        format!("__w::de::counted(b, {}, at, {quoted})?", count.bytes()),
                        Vec::new(),
                    ),
                    Sequence::Prefixed { count, prefix } => {
                        let mut check = Vec::new();
                        if let Some(key) = key {
                            check.push(format!("__w::de::at_least(b, {key_size}, at, {quoted})?;"));
                            let key_check = self.check_fixed(key, label, module);
                            if !key_check.is_empty() {
                                check.push("{".to_string());
                                check.push(format!("let (b, at) = (&b[..{key_size}], at);"));
                                check.extend(key_check);
                                check.push("}".to_string());
                            }
                            let value_check = self.check_var(item, label, module);
                            if !value_check.is_empty() {
                                check.push("{".to_string());
                                check.push(format!(
                                    "let (b, at) = (&b[{key_size}..], at + {key_size});"
                                ));
                                check.extend(value_check);
                                check.push("}".to_string());
                            }
                        } else {
                            check = self.check_var(item, label, module);
                        }
                        let call = if check.is_empty() {
                            format!(
                                "__w::de::prefixed(b, {}, {}, at, {quoted}, |_, _| Ok(()))?",
                                count.bytes(),
                                prefix.bytes()
                            )
                        } else {
                            let mut call = format!(
                                "__w::de::prefixed(b, {}, {}, at, {quoted}, |b, at| {{\n",
                                count.bytes(),
                                prefix.bytes()
                            );
                            for line in check {
                                call.push_str(&line);
                                call.push('\n');
                            }
                            call.push_str("Ok(())\n})?");
                            call
                        };
                        (call, Vec::new())
                    }
                };
                // Single bytes always fill their range.
                let trivial = matches!(self.wire.sequence(ty), Some(Sequence::Packed { item: 1 }));
                if length.is_some() || !body.is_empty() {
                    lines.push(format!("let n = {count};"));
                } else if !trivial {
                    lines.push(format!("{count};"));
                }
                if let Some(args) = length {
                    lines.push(format!("__w::de::length(n, {args}, at, {quoted})?;"));
                }
                lines.extend(body);
            }
            TypeKind::Named(id) => {
                let view = match self.schema.def(*id).kind {
                    DefKind::Union(_) => self.union_view_ty(*id, module, "'_"),
                    _ => format!("{}<'_>", self.view_path(*id, module)),
                };
                lines.push(format!("<{view} as __w::Validate>::validate_at(b, at)?;"));
            }
            _ => unreachable!("only strings, bytes, collections, structs, and unions vary in size"),
        }
        lines
    }

    /// Whether a fixed-size struct needs checks beyond its size.
    pub(super) fn is_checked(&self, id: TypeId) -> bool {
        if let Some(checked) = self.checked.borrow()[id.0] {
            return checked;
        }
        let layout = self.wire.layout(id);
        let fields = match &self.schema.def(id).kind {
            DefKind::Struct(item) => &item.fields,
            DefKind::Message(item) => &item.fields,
            DefKind::Enum(_) | DefKind::Type(_) => {
                unreachable!("enums and types are checked as what they hold")
            }
            DefKind::Union(_) => unreachable!("a union is never fixed-size"),
        };
        let bits: u32 = layout
            .fields
            .iter()
            .map(|slot| match slot {
                Slot::Bit(_) => 1,
                Slot::OptionalBit { .. } => 2,
                _ => 0,
            })
            .sum();
        let module = &self.modules[id.0];
        let checked = !bits.is_multiple_of(8)
            || layout
                .fields
                .iter()
                .any(|slot| matches!(slot, Slot::OptionalBit { .. }))
            || fields.iter().zip(&layout.fields).any(|(field, slot)| {
                matches!(slot, Slot::At(_)) && !self.check_fixed(&field.ty, "", module).is_empty()
            });
        self.checked.borrow_mut()[id.0] = Some(checked);
        checked
    }

    /// Reads bit `bit` of `b` as `ty`, a `bool` or a `type` over one.
    pub(super) fn bit(&self, ty: &Type, bit: u32, module: &[String]) -> String {
        let mut read = format!("__w::bit(b, {bit})");
        for path in self.newtypes(ty, module).iter().rev() {
            read = format!("{path}({read})");
        }
        read
    }

    /// The `(presence, size)` list of an object's optional fixed-size
    /// fields, as `__w::offset` takes it.
    pub(super) fn optionals(&self, object: &Object) -> String {
        let optionals: Vec<String> = object
            .fields
            .iter()
            .zip(&object.layout.fields)
            .filter_map(|(field, slot)| match slot {
                Slot::Optional { presence } => {
                    Some(format!("({presence}, {})", self.fixed_size(&field.ty)))
                }
                _ => None,
            })
            .collect();
        optionals.join(", ")
    }
}

impl Generator<'_> {
    /// A view's `decode`, `decode_unchecked`, and `validate`: `unchecked`
    /// makes `output` from `buf`, and `validated` names the view's `impl
    /// __w::Validate`.
    pub(super) fn decoders(
        &self,
        out: &mut Out,
        name: &str,
        input: &str,
        output: &str,
        unchecked: &str,
        validated: &str,
    ) {
        out.line("/// Validates `buf` as exactly one encoding.");
        out.line(format!(
            "pub fn decode(buf: {input}) -> {RESULT}<{output}, __w::DecodeError> {{"
        ));
        out.line("Self::validate(buf)?;");
        out.line("// SAFETY: just validated.");
        out.line("Ok(unsafe { Self::decode_unchecked(buf) })");
        out.line("}");
        out.line("");
        out.line("/// # Safety");
        out.line("///");
        out.line("/// `buf` must hold exactly one encoding [`Self::validate`] accepts.");
        out.line(format!(
            "pub unsafe fn decode_unchecked(buf: {input}) -> {output} {{"
        ));
        out.line(unchecked);
        out.line("}");
        out.line("");
        out.line(format!(
            "pub fn validate(buf: &[u8]) -> {RESULT}<(), __w::DecodeError> {{"
        ));
        out.line(format!("__w::de::size(buf, {name:?})?;"));
        out.line(format!(
            "<{validated} as __w::Validate>::validate_at(buf, 0)"
        ));
        out.line("}");
    }

    /// `name { .. }` built from the accessors of `receiver`, a view of
    /// `object`.
    pub(super) fn owned_object(&self, object: &Object, name: &str, receiver: &str) -> String {
        if object.fields.is_empty() {
            return format!("{name} {{}}");
        }
        let mut lines = vec![format!("{name} {{")];
        for field in object.fields {
            let read = format!("{receiver}.{}()", accessor(&field.name));
            let owned = if field.optional {
                optional(&read, &self.owned_from(&field.ty, "v"))
            } else {
                self.owned_from(&field.ty, &read)
            };
            lines.push(format!("{}: {owned},", ident(&field.name)));
        }
        lines.push("}".to_string());
        lines.join("\n")
    }

    /// Whether union `id`'s view borrows the buffer: a variant's data does.
    /// One that does not takes no lifetime, which rustc would reject as
    /// unused.
    pub(super) fn union_borrows(&self, id: TypeId) -> bool {
        if let Some(borrows) = self.borrows.borrow()[id.0] {
            return borrows;
        }
        let DefKind::Union(item) = &self.schema.def(id).kind else {
            unreachable!("a union");
        };
        let module = &self.modules[id.0];
        let borrows = item.variants.iter().any(|variant| match &variant.data {
            VariantData::Unit => false,
            VariantData::Type(ty) => self.view_ty(ty, module, "'a").contains("'a"),
            VariantData::Fields(_) => true,
        });
        self.borrows.borrow_mut()[id.0] = Some(borrows);
        borrows
    }

    /// The view of union `id` as seen from `module`, borrowing for `lt`.
    pub(super) fn union_view_ty(&self, id: TypeId, module: &[String], lt: &str) -> String {
        let view = self.view_path(id, module);
        if self.union_borrows(id) {
            format!("{view}<{lt}>")
        } else {
            view
        }
    }

    /// The view of union `id`: an enum of what each variant's data reads
    /// as, a variant's fields through its view in `snake(Union)`.
    pub(super) fn union_view(&self, out: &mut Out, id: TypeId, item: &Union, module: &[String]) {
        let layout = self.wire.union(id);
        let name = &self.names[id.0];
        let label = self.schema.path(id).join(".");
        let view = format!("{name}View");
        let borrows = self.union_borrows(id);
        let (ty, generics, input) = if borrows {
            (format!("{view}<'a>"), "<'a>", "&'a [u8]")
        } else {
            (view.clone(), "", "&[u8]")
        };
        let anonymous = if borrows {
            format!("{view}<'_>")
        } else {
            view.clone()
        };
        let tag = layout.tag.bytes();
        let tag_cell = match layout.tag {
            Width::U8 => "u8",
            _ => "__w::U16",
        };
        let inner = snake(&self.schema.def(id).name);
        // What each variant's data reads as, and whether it is fixed-size.
        let payloads: Vec<Option<(String, bool)>> = item
            .variants
            .iter()
            .enumerate()
            .map(|(index, variant)| match &variant.data {
                VariantData::Unit => None,
                VariantData::Type(ty) => Some((
                    self.view_ty(ty, module, "'a"),
                    self.wire.size(ty).fixed().is_some(),
                )),
                VariantData::Fields(_) => {
                    let path = format!("self::{inner}::{}View", variant.name);
                    let fixed = layout.variants[index]
                        .as_ref()
                        .expect("a variant with fields")
                        .size
                        .fixed()
                        .is_some();
                    let ty = if fixed {
                        format!("&'a {path}")
                    } else {
                        format!("{path}<'a>")
                    };
                    Some((ty, fixed))
                }
            })
            .collect();

        out.line(format!("/// [`{name}`], read in place."));
        out.line("#[derive(Debug, Clone, Copy)]");
        out.line(format!("pub enum {ty} {{"));
        for (variant, payload) in item.variants.iter().zip(&payloads) {
            let variant = ident(&variant.name);
            match payload {
                None => out.line(format!("{variant},")),
                Some((ty, _)) => out.line(format!("{variant}({ty}),")),
            }
        }
        out.line("}");

        out.line("");
        out.line(format!("impl{generics} {ty} {{"));
        self.decoders(
            out,
            &label,
            input,
            "Self",
            "unsafe { <Self as __w::Var<'_>>::read(buf) }",
            &anonymous,
        );
        out.line("");
        out.line(format!("pub fn to_owned(&self) -> {name} {{"));
        out.line("match *self {");
        for (index, variant) in item.variants.iter().enumerate() {
            let variant_name = ident(&variant.name);
            let owned = format!("{name}::{variant_name}");
            match &variant.data {
                VariantData::Unit => out.line(format!("Self::{variant_name} => {owned},")),
                VariantData::Type(ty) => out.line(format!(
                    "Self::{variant_name}(v) => {owned}({}),",
                    self.owned_from(ty, "v")
                )),
                VariantData::Fields(fields) => {
                    let object = self.variant_object(id, index, fields, module);
                    out.line(format!(
                        "Self::{variant_name}(v) => {},",
                        self.owned_object(&object, &owned, "v")
                    ));
                }
            }
        }
        out.line("}");
        out.line("}");
        out.line("}");

        out.line("");
        out.line(format!("impl __w::Validate for {anonymous} {{"));
        out.line(format!(
            "fn validate_at(buf: &[u8], at: usize) -> {RESULT}<(), __w::DecodeError> {{"
        ));
        out.line(format!("__w::de::at_least(buf, {tag}, at, {label:?})?;"));
        let mut arms = Vec::new();
        for (index, (variant, payload)) in item.variants.iter().zip(&payloads).enumerate() {
            let quoted = format!("{label}.{}", variant.name);
            let check = match (&variant.data, payload) {
                (VariantData::Unit, _) => {
                    vec![format!("__w::de::exact(b, 0, at, {quoted:?})?;")]
                }
                (VariantData::Type(ty), Some((_, true))) => {
                    let mut check = vec![format!(
                        "__w::de::exact(b, {}, at, {quoted:?})?;",
                        self.fixed_size(ty)
                    )];
                    check.extend(self.check_fixed(ty, &quoted, module));
                    check
                }
                (VariantData::Type(ty), _) => self.check_var(ty, &quoted, module),
                (VariantData::Fields(_), Some((ty, _))) => {
                    let view = ty.trim_start_matches("&'a ").replace("<'a>", "<'_>");
                    vec![format!("<{view} as __w::Validate>::validate_at(b, at)?;")]
                }
                (VariantData::Fields(_), None) => unreachable!("fields read as a view"),
            };
            arms.push((UnionLayout::tag_of(index), check));
        }
        // Each arm checks `b`, the variant's data, starting `at`.
        let reads = arms.iter().any(|(_, check)| !check.is_empty());
        if reads {
            out.line(format!("let (b, at) = (&buf[{tag}..], at + {tag});"));
        }
        out.line(format!("match __w::de::plain::<{tag_cell}>(buf, 0) {{"));
        for (tag, check) in arms {
            if check.is_empty() {
                out.line(format!("{tag} => {{}}"));
            } else {
                out.line(format!("{tag} => {{"));
                out.extend(check);
                out.line("}");
            }
        }
        let at = if reads {
            format!("at - {tag}")
        } else {
            "at".into()
        };
        out.line(format!("_ => return __w::de::tag({at}, {label:?}),"));
        out.line("}");
        out.line("Ok(())");
        out.line("}");
        out.line("}");

        out.line("");
        out.line(format!("impl<'a> __w::Var<'a> for {view}{generics} {{"));
        out.line("unsafe fn read(bytes: &'a [u8]) -> Self {");
        if payloads.iter().any(Option::is_some) {
            out.line(format!("let b = &bytes[{tag}..];"));
        }
        out.line(format!("match __w::de::plain::<{tag_cell}>(bytes, 0) {{"));
        for (index, (variant, payload)) in item.variants.iter().zip(&payloads).enumerate() {
            let tag = UnionLayout::tag_of(index);
            let variant_name = ident(&variant.name);
            let Some((ty, fixed)) = payload else {
                out.line(format!("{tag} => Self::{variant_name},"));
                continue;
            };
            let read = match (&variant.data, fixed) {
                (VariantData::Fields(_), true) => {
                    format!("__w::cast::<{}>(b)", ty.trim_start_matches("&'a "))
                }
                (VariantData::Type(ty), true) => {
                    format!("__w::read::<{}>(b, 0)", self.cell(ty, module))
                }
                _ => format!("<{ty} as __w::Var<'a>>::read(b)"),
            };
            out.line(format!(
                "{tag} => Self::{variant_name}(unsafe {{ {read} }}),"
            ));
        }
        out.line("// SAFETY: validated as a declared tag.");
        out.line("_ => unsafe { ::core::hint::unreachable_unchecked() },");
        out.line("}");
        out.line("}");
        out.line("}");

        out.line("");
        let target = if borrows {
            format!("{view}<'a>")
        } else {
            view.clone()
        };
        decode_impl(out, &target, &view);
    }
}

/// `impl __w::Decode` for `target`, a view or a reference to one, through
/// `view`'s `decode`.
fn decode_impl(out: &mut Out, target: &str, view: &str) {
    out.line(format!("impl<'a> __w::Decode<'a> for {target} {{"));
    out.line(format!(
        "fn decode(buf: &'a [u8]) -> {RESULT}<Self, __w::DecodeError> {{"
    ));
    out.line(format!("{view}::decode(buf)"));
    out.line("}");
    out.line("}");
}
