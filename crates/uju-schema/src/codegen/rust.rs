use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};

use crate::codegen::{Backend, GeneratedFile};
use crate::ir::{
    ConstDef, ConstValue, EnumDef, FieldDef, Name, Prim, RecordDef, RecordKind, Schema, Size, Ty,
    TypeDef,
};

pub struct Rust;

impl Backend for Rust {
    fn name(&self) -> &str {
        "rust"
    }

    fn emit(&self, schema: &Schema) -> Vec<GeneratedFile> {
        let mut modules: BTreeSet<Vec<String>> = BTreeSet::new();
        for path in schema
            .types
            .iter()
            .map(|t| module_path(t.name()))
            .chain(schema.consts.iter().map(|c| module_path(&c.name)))
        {
            for depth in 0..=path.len() {
                modules.insert(path[..depth].to_vec());
            }
        }

        let mut contents = format!("pub const SCHEMA_HASH: u64 = {:#018x};\n", schema.hash());
        contents.push_str(&module(schema, &modules, &[]));
        vec![GeneratedFile {
            path: PathBuf::from("schema.rs"),
            contents,
        }]
    }
}

fn module(schema: &Schema, modules: &BTreeSet<Vec<String>>, path: &[String]) -> String {
    let mut out = String::new();
    let types: Vec<&TypeDef> = schema
        .types
        .iter()
        .filter(|t| module_path(t.name()) == path)
        .collect();
    let consts: Vec<&ConstDef> = schema
        .consts
        .iter()
        .filter(|c| module_path(&c.name) == path)
        .collect();

    if !types.is_empty() {
        out.push_str("\n#[allow(unused_imports)]\nuse ::uju::wire::{View as _, Wire as _};\n");
    }
    for def in &consts {
        out.push('\n');
        out.push_str(&const_def(schema, path, def));
    }
    for def in &types {
        out.push('\n');
        match def {
            TypeDef::Enum(def) => out.push_str(&enum_def(def)),
            TypeDef::Record(def) => out.push_str(&record_def(schema, path, def)),
        }
    }

    for child in modules.iter() {
        if child.len() != path.len() + 1 || !child.starts_with(path) {
            continue;
        }
        let body = module(schema, modules, child);
        let _ = write!(
            out,
            "\npub mod {} {{{}}}\n",
            child.last().unwrap(),
            indent(&body)
        );
    }
    out
}

fn indent(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = write!(out, "\n    {line}");
        }
    }
    out.push('\n');
    out
}

fn enum_def(def: &EnumDef) -> String {
    let name = type_ident(&def.name.name);
    let repr = prim_ty(def.repr);
    let size = def.repr.size().unwrap();
    let mut out = String::new();

    let _ = write!(
        out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\n\
         #[repr({repr})]\n\
         pub enum {name} {{\n"
    );
    for variant in &def.variants {
        let _ = write!(
            out,
            "    {} = {},\n",
            variant_ident(&variant.name),
            variant.value
        );
    }
    out.push_str("}\n\n");

    let _ = write!(
        out,
        "impl {name} {{\n    pub const VARIANTS: [{name}; {}] = [",
        def.variants.len()
    );
    for (i, variant) in def.variants.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{name}::{}", variant_ident(&variant.name));
    }
    out.push_str("];\n\n");

    let _ = write!(
        out,
        "    pub fn from_repr(value: {repr}) -> Option<Self> {{\n        match value {{\n"
    );
    for variant in &def.variants {
        let _ = write!(
            out,
            "            {} => Some({name}::{}),\n",
            variant.value,
            variant_ident(&variant.name)
        );
    }
    out.push_str("            _ => None,\n        }\n    }\n\n");
    let _ = write!(
        out,
        "    pub fn to_repr(self) -> {repr} {{\n        self as {repr}\n    }}\n}}\n\n"
    );

    let _ = write!(
        out,
        "impl ::uju::wire::Wire for {name} {{\n\
         \x20   const FIXED_SIZE: Option<usize> = Some({size});\n\n\
         \x20   fn encoded_size(&self) -> usize {{\n        {size}\n    }}\n\n\
         \x20   fn encode(&self, w: &mut ::uju::wire::Writer) {{\n        w.push_{repr}(self.to_repr());\n    }}\n}}\n\n"
    );

    let fallback = match def.variants.first() {
        Some(first) => format!("{name}::{}", variant_ident(&first.name)),
        None => "unreachable!()".to_string(),
    };
    let _ = write!(
        out,
        "impl<'a> ::uju::wire::View<'a> for {name} {{\n\
         \x20   type Owned = Self;\n\n\
         \x20   const FIXED_SIZE: Option<usize> = Some({size});\n\n\
         \x20   fn read(bytes: &'a [u8]) -> Self {{\n        \
         Self::from_repr(::uju::wire::read_{repr}(bytes, 0)).unwrap_or({fallback})\n    }}\n\n\
         \x20   fn validate(bytes: &'a [u8]) -> ::uju::wire::Result<usize> {{\n        \
         ::uju::wire::need(bytes, {size})?;\n        \
         match Self::from_repr(::uju::wire::read_{repr}(bytes, 0)) {{\n            \
         Some(_) => Ok({size}),\n            None => Err(::uju::wire::Error::BadEnum),\n        }}\n    }}\n\n\
         \x20   fn owned(self) -> Self {{\n        self\n    }}\n}}\n\n"
    );

    let _ = write!(
        out,
        "impl ::uju::wire::Canonical for {name} {{\n    \
         fn canonical_cmp(&self, other: &Self) -> core::cmp::Ordering {{\n        \
         self.to_repr().cmp(&other.to_repr())\n    }}\n}}\n"
    );
    out
}

fn record_def(schema: &Schema, path: &[String], def: &RecordDef) -> String {
    let name = type_ident(&def.name.name);
    let view = format!("{name}Ref");
    let fixed = def.layout.fixed_size;
    let wire_size = match def.layout.size {
        Size::Fixed(n) => format!("Some({n})"),
        Size::Variable => "None".to_string(),
    };
    let copy = matches!(def.layout.size, Size::Fixed(_));
    let mut out = String::new();

    let derives = if copy {
        "#[derive(Debug, Clone, Copy, PartialEq)]"
    } else {
        "#[derive(Debug, Clone, PartialEq)]"
    };
    let _ = write!(out, "{derives}\npub struct {name} {{\n");
    for field in &def.fields {
        let _ = write!(
            out,
            "    pub {}: {},\n",
            field_ident(&field.name),
            wrap_optional(field, owned_ty(schema, path, &field.ty))
        );
    }
    out.push_str("}\n\n");

    let _ = write!(
        out,
        "impl {name} {{\n\
         \x20   pub const FIXED_SIZE: usize = {fixed};\n\
         \x20   pub const BITMAP_BYTES: usize = {};\n\
         \x20   pub const WIRE_SIZE: Option<usize> = {wire_size};\n\n\
         \x20   pub fn view(bytes: &[u8]) -> {view}<'_> {{\n        {view}::read(bytes)\n    }}\n\n\
         \x20   pub fn validate(bytes: &[u8]) -> ::uju::wire::Result<usize> {{\n        \
         <{view} as ::uju::wire::View>::validate(bytes)\n    }}\n\n\
         \x20   pub fn decode(bytes: &[u8]) -> Self {{\n        {view}::read(bytes).owned()\n    }}\n\n\
         \x20   pub fn encode_to_vec(&self) -> ::uju::wire::Result<Vec<u8>> {{\n        \
         ::uju::wire::encode(self)\n    }}\n}}\n\n",
        def.layout.bitmap_bytes
    );

    out.push_str(&wire_impl(schema, def, &name));
    out.push_str(&canonical_impl(
        &name,
        def.fields
            .iter()
            .map(|f| format!("self.{}", field_ident(&f.name))),
        def.fields
            .iter()
            .map(|f| format!("other.{}", field_ident(&f.name))),
    ));

    if let Some(info) = def.message {
        let _ = write!(
            out,
            "\nimpl ::uju::wire::Message for {name} {{\n    const MESSAGE_ID: u32 = {};\n}}\n",
            info.id
        );
        if let Some(response) = info.returns {
            let _ = write!(
                out,
                "\nimpl ::uju::wire::Request for {name} {{\n    type Response = {};\n}}\n",
                type_path(path, schema.type_def(response).name())
            );
        }
    }
    if def.kind == RecordKind::Component {
        let _ = write!(out, "\nimpl ::uju::wire::Component for {name} {{}}\n");
    }

    out.push('\n');
    out.push_str(&view_def(schema, path, def, &name, &view));
    out
}

fn wire_impl(schema: &Schema, def: &RecordDef, name: &str) -> String {
    let fixed = def.layout.fixed_size;
    let mut out = String::new();
    let wire_size = match def.layout.size {
        Size::Fixed(n) => format!("Some({n})"),
        Size::Variable => "None".to_string(),
    };

    let _ = write!(
        out,
        "impl ::uju::wire::Wire for {name} {{\n    const FIXED_SIZE: Option<usize> = {wire_size};\n\n"
    );

    out.push_str("    fn encoded_size(&self) -> usize {\n");
    let var_fields: Vec<&FieldDef> = def
        .fields
        .iter()
        .filter(|f| is_variable(schema, &f.ty))
        .collect();
    if var_fields.is_empty() {
        let _ = write!(out, "        {fixed}\n");
    } else {
        let _ = write!(out, "        let mut size = {fixed}usize;\n");
        for field in &var_fields {
            let value = field_ident(&field.name);
            if field.optional {
                let _ = write!(
                    out,
                    "        if let Some(value) = &self.{value} {{\n            \
                     size += ::uju::wire::Wire::encoded_size(value);\n        }}\n"
                );
            } else {
                let _ = write!(
                    out,
                    "        size += ::uju::wire::Wire::encoded_size(&self.{value});\n"
                );
            }
        }
        out.push_str("        size\n");
    }
    out.push_str("    }\n\n");

    out.push_str("    fn encode(&self, w: &mut ::uju::wire::Writer) {\n");
    if def.layout.size == Size::Variable {
        out.push_str("        let start = w.pos();\n");
    }
    if def.layout.bitmap_bytes > 0 {
        let _ = write!(
            out,
            "        let mut bitmap = [0u8; {}];\n",
            def.layout.bitmap_bytes
        );
        for field in def.fields.iter().filter(|f| f.bit.is_some()) {
            let bit = field.bit.unwrap();
            let _ = write!(
                out,
                "        if self.{}.is_some() {{\n            bitmap[{}] |= 1 << {};\n        }}\n",
                field_ident(&field.name),
                bit / 8,
                bit % 8
            );
        }
        out.push_str("        w.push_bytes(&bitmap);\n");
    }

    let mut slots = Vec::new();
    for field in &def.fields {
        let value = field_ident(&field.name);
        if is_variable(schema, &field.ty) {
            let slot = format!("slot_{value}");
            let _ = write!(
                out,
                "        let {slot} = w.pos();\n        w.push_u16(0);\n"
            );
            slots.push((field, slot));
        } else if field.optional {
            let size = fixed_size(schema, &field.ty);
            let _ = write!(
                out,
                "        match &self.{value} {{\n            \
                 Some(value) => ::uju::wire::Wire::encode(value, w),\n            \
                 None => w.push_zeros({size}),\n        }}\n"
            );
        } else {
            let _ = write!(
                out,
                "        ::uju::wire::Wire::encode(&self.{value}, w);\n"
            );
        }
    }
    for (field, slot) in &slots {
        let value = field_ident(&field.name);
        if field.optional {
            let _ = write!(
                out,
                "        if let Some(value) = &self.{value} {{\n            \
                 let offset = w.short(w.pos() - start);\n            \
                 w.put_u16({slot}, offset);\n            \
                 ::uju::wire::Wire::encode(value, w);\n        }}\n"
            );
        } else {
            let _ = write!(
                out,
                "        let offset = w.short(w.pos() - start);\n        \
                 w.put_u16({slot}, offset);\n        \
                 ::uju::wire::Wire::encode(&self.{value}, w);\n"
            );
        }
    }
    out.push_str("    }\n}\n");
    out
}

fn view_def(schema: &Schema, path: &[String], def: &RecordDef, name: &str, view: &str) -> String {
    let mut out = String::new();
    let wire_size = match def.layout.size {
        Size::Fixed(n) => format!("Some({n})"),
        Size::Variable => "None".to_string(),
    };

    let _ = write!(
        out,
        "#[derive(Clone, Copy)]\npub struct {view}<'a> {{\n    bytes: &'a [u8],\n}}\n\n\
         impl<'a> {view}<'a> {{\n    pub fn as_bytes(self) -> &'a [u8] {{\n        self.bytes\n    }}\n"
    );
    for field in &def.fields {
        let ty = wrap_optional(field, view_ty(schema, path, &field.ty));
        let _ = write!(
            out,
            "\n    pub fn {}(self) -> {ty} {{\n{}    }}\n",
            field_ident(&field.name),
            accessor_body(schema, path, field)
        );
    }
    out.push_str("}\n\n");

    let _ = write!(
        out,
        "impl<'a> ::uju::wire::View<'a> for {view}<'a> {{\n\
         \x20   type Owned = {name};\n\n\
         \x20   const FIXED_SIZE: Option<usize> = {wire_size};\n\n\
         \x20   fn read(bytes: &'a [u8]) -> Self {{\n        Self {{ bytes }}\n    }}\n\n"
    );
    out.push_str(&validate_body(schema, path, def));
    let _ = write!(
        out,
        "\n    fn owned(self) -> {name} {{\n        {name} {{\n"
    );
    for field in &def.fields {
        let accessor = field_ident(&field.name);
        let value = if is_owned_by_value(schema, &field.ty) {
            format!("self.{accessor}()")
        } else if field.optional {
            format!("self.{accessor}().map(::uju::wire::View::owned)")
        } else {
            format!("::uju::wire::View::owned(self.{accessor}())")
        };
        let _ = write!(out, "            {accessor}: {value},\n");
    }
    out.push_str("        }\n    }\n}\n\n");

    out.push_str(&canonical_impl(
        &format!("{view}<'a>"),
        def.fields
            .iter()
            .map(|f| format!("self.{}()", field_ident(&f.name))),
        def.fields
            .iter()
            .map(|f| format!("other.{}()", field_ident(&f.name))),
    ));
    out
}

fn accessor_body(schema: &Schema, path: &[String], field: &FieldDef) -> String {
    let offset = field.offset;
    let read = read_expr(schema, path, &field.ty, &format!("{offset}"));
    match (field.optional, is_variable(schema, &field.ty)) {
        (false, _) => format!("        {read}\n"),
        (true, false) => {
            let bit = field.bit.unwrap();
            format!(
                "        if ::uju::wire::test_bit(self.bytes, 0, {bit}) {{\n            \
                 Some({read})\n        }} else {{\n            None\n        }}\n"
            )
        }
        (true, true) => {
            let inner = read_expr(schema, path, &field.ty, "at");
            format!(
                "        let at = ::uju::wire::read_u16(self.bytes, {offset}) as usize;\n        \
                 if at == 0 {{\n            None\n        }} else {{\n            \
                 Some({inner})\n        }}\n"
            )
        }
    }
}

fn read_expr(schema: &Schema, path: &[String], ty: &Ty, at: &str) -> String {
    if is_variable(schema, ty) {
        let target = view_ty(schema, path, ty);
        return format!(
            "<{target} as ::uju::wire::View>::read(&self.bytes[::uju::wire::read_u16(self.bytes, {at}) as usize..])"
        );
    }
    match ty {
        Ty::Prim(p) => match p {
            Prim::U8 => format!("::uju::wire::read_u8(self.bytes, {at})"),
            Prim::I8 => format!("::uju::wire::read_i8(self.bytes, {at})"),
            Prim::Bool => format!("::uju::wire::read_bool(self.bytes, {at})"),
            _ => format!(
                "<{} as ::uju::wire::View>::read(&self.bytes[{at}..])",
                prim_view_ty(*p)
            ),
        },
        _ => format!(
            "<{} as ::uju::wire::View>::read(&self.bytes[{at}..])",
            view_ty(schema, path, ty)
        ),
    }
}

fn validate_body(schema: &Schema, path: &[String], def: &RecordDef) -> String {
    let fixed = def.layout.fixed_size;
    let mut out = String::new();
    out.push_str("    fn validate(bytes: &'a [u8]) -> ::uju::wire::Result<usize> {\n");
    let _ = write!(out, "        ::uju::wire::need(bytes, {fixed})?;\n");

    let optional_fixed = def.fields.iter().filter(|f| f.bit.is_some()).count();
    if optional_fixed % 8 != 0 {
        let mask = 0xffu8 << (optional_fixed % 8);
        let _ = write!(
            out,
            "        if bytes[{}] & {mask:#04x} != 0 {{\n            \
             return Err(::uju::wire::Error::BadPadding);\n        }}\n",
            def.layout.bitmap_bytes - 1
        );
    }

    for field in &def.fields {
        if is_variable(schema, &field.ty) {
            continue;
        }
        let offset = field.offset;
        let size = fixed_size(schema, &field.ty);
        let check = needs_check(schema, &field.ty).then(|| {
            format!(
                "<{} as ::uju::wire::View>::validate(&bytes[{offset}..])?;",
                view_ty(schema, path, &field.ty)
            )
        });
        match (field.optional, check) {
            (false, Some(check)) => {
                let _ = write!(out, "        {check}\n");
            }
            (false, None) => {}
            (true, check) => {
                let bit = field.bit.unwrap();
                let _ = write!(
                    out,
                    "        if ::uju::wire::test_bit(bytes, 0, {bit}) {{\n            {}\n        }} \
                     else if !::uju::wire::is_zero(bytes, {offset}, {size}) {{\n            \
                     return Err(::uju::wire::Error::BadPadding);\n        }}\n",
                    check.unwrap_or_default()
                );
            }
        }
    }

    let var_fields: Vec<&FieldDef> = def
        .fields
        .iter()
        .filter(|f| is_variable(schema, &f.ty))
        .collect();
    if var_fields.is_empty() {
        let _ = write!(out, "        Ok({fixed})\n    }}\n");
        return out;
    }

    let _ = write!(out, "        let mut cursor = {fixed}usize;\n");
    for field in &var_fields {
        let offset = field.offset;
        let target = view_ty(schema, path, &field.ty);
        let step =
            format!("cursor += <{target} as ::uju::wire::View>::validate(&bytes[cursor..])?;");
        let _ = write!(
            out,
            "        let at = ::uju::wire::read_u16(bytes, {offset}) as usize;\n"
        );
        if field.optional {
            let _ = write!(
                out,
                "        if at != 0 {{\n            if at != cursor {{\n                \
                 return Err(::uju::wire::Error::BadOffset);\n            }}\n            {step}\n        }}\n"
            );
        } else {
            let _ = write!(
                out,
                "        if at != cursor {{\n            \
                 return Err(::uju::wire::Error::BadOffset);\n        }}\n        {step}\n"
            );
        }
    }
    out.push_str("        Ok(cursor)\n    }\n");
    out
}

fn canonical_impl(
    target: &str,
    left: impl Iterator<Item = String>,
    right: impl Iterator<Item = String>,
) -> String {
    let generics = if target.contains('\'') { "<'a>" } else { "" };
    let mut out = String::new();
    let _ = write!(
        out,
        "impl{generics} ::uju::wire::Canonical for {target} {{\n    \
         fn canonical_cmp(&self, other: &Self) -> core::cmp::Ordering {{\n        \
         core::cmp::Ordering::Equal\n"
    );
    for (a, b) in left.zip(right) {
        let _ = write!(
            out,
            "            .then_with(|| ::uju::wire::Canonical::canonical_cmp(&{a}, &{b}))\n"
        );
    }
    out.push_str("    }\n}\n");
    out
}

fn const_def(schema: &Schema, path: &[String], def: &ConstDef) -> String {
    let name = def.name.name.to_shouty_snake_case();
    let ty = match &def.ty {
        Ty::Prim(Prim::String) => "&str".to_string(),
        ty => owned_ty(schema, path, ty),
    };
    let value = match &def.value {
        ConstValue::Int(v) => match &def.ty {
            Ty::Prim(Prim::Timestamp) => format!("::uju::wire::Timestamp({v})"),
            Ty::Prim(Prim::Interval) => format!("::uju::wire::Interval({v})"),
            _ => v.to_string(),
        },
        ConstValue::Float(v) => format!("{v:?}"),
        ConstValue::Bool(v) => v.to_string(),
        ConstValue::String(v) => format!("{v:?}"),
        ConstValue::Variant { ty, index } => {
            let TypeDef::Enum(def) = schema.type_def(*ty) else {
                unreachable!()
            };
            format!(
                "{}::{}",
                type_path(path, &def.name),
                variant_ident(&def.variants[*index as usize].name)
            )
        }
    };
    format!("pub const {name}: {ty} = {value};\n")
}

fn module_path(name: &Name) -> Vec<String> {
    name.namespace
        .iter()
        .map(|s| mod_ident(s))
        .chain(name.scope.iter().map(|s| mod_ident(s)))
        .collect()
}

fn type_path(from: &[String], target: &Name) -> String {
    let to = module_path(target);
    let name = type_ident(&target.name);
    if to == from {
        return name;
    }
    let mut out = "super::".repeat(from.len());
    for segment in &to {
        out.push_str(segment);
        out.push_str("::");
    }
    out.push_str(&name);
    out
}

fn owned_ty(schema: &Schema, path: &[String], ty: &Ty) -> String {
    match ty {
        Ty::Prim(Prim::String) => "String".to_string(),
        Ty::Prim(Prim::Bytes) => "Vec<u8>".to_string(),
        Ty::Prim(p) => prim_ty(*p).to_string(),
        Ty::Ref(id) => type_path(path, schema.type_def(*id).name()),
        Ty::Vec(t) => format!("Vec<{}>", owned_ty(schema, path, t)),
        Ty::Set(t) => format!("::uju::wire::Set<{}>", owned_ty(schema, path, t)),
        Ty::Map(k, v) => format!(
            "::uju::wire::Map<{}, {}>",
            owned_ty(schema, path, k),
            owned_ty(schema, path, v)
        ),
    }
}

fn view_ty(schema: &Schema, path: &[String], ty: &Ty) -> String {
    match ty {
        Ty::Prim(p) => prim_view_ty(*p),
        Ty::Ref(id) => {
            let def = schema.type_def(*id);
            match def {
                TypeDef::Enum(_) => type_path(path, def.name()),
                TypeDef::Record(_) => format!("{}Ref<'a>", type_path(path, def.name())),
            }
        }
        Ty::Vec(t) => format!("::uju::wire::VecView<'a, {}>", view_ty(schema, path, t)),
        Ty::Set(t) => format!("::uju::wire::SetView<'a, {}>", view_ty(schema, path, t)),
        Ty::Map(k, v) => format!(
            "::uju::wire::MapView<'a, {}, {}>",
            view_ty(schema, path, k),
            view_ty(schema, path, v)
        ),
    }
}

fn prim_ty(prim: Prim) -> &'static str {
    match prim {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        Prim::F32 => "f32",
        Prim::F64 => "f64",
        Prim::Bool => "bool",
        Prim::Timestamp => "::uju::wire::Timestamp",
        Prim::Interval => "::uju::wire::Interval",
        Prim::Entity => "::uju::wire::Entity",
        Prim::UEntity => "::uju::wire::UEntity",
        Prim::String => "String",
        Prim::Bytes => "Vec<u8>",
    }
}

fn prim_view_ty(prim: Prim) -> String {
    match prim {
        Prim::String => "&'a str".to_string(),
        Prim::Bytes => "&'a [u8]".to_string(),
        p => prim_ty(p).to_string(),
    }
}

fn wrap_optional(field: &FieldDef, ty: String) -> String {
    if field.optional {
        format!("Option<{ty}>")
    } else {
        ty
    }
}

fn is_variable(schema: &Schema, ty: &Ty) -> bool {
    ty.size(schema) == Size::Variable
}

fn fixed_size(schema: &Schema, ty: &Ty) -> u32 {
    match ty.size(schema) {
        Size::Fixed(n) => n,
        Size::Variable => 0,
    }
}

fn is_owned_by_value(schema: &Schema, ty: &Ty) -> bool {
    match ty {
        Ty::Prim(Prim::String | Prim::Bytes) => false,
        Ty::Prim(_) => true,
        Ty::Ref(id) => matches!(schema.type_def(*id), TypeDef::Enum(_)),
        _ => false,
    }
}

fn needs_check(schema: &Schema, ty: &Ty) -> bool {
    match ty {
        Ty::Prim(Prim::Bool) => true,
        Ty::Prim(_) => false,
        Ty::Ref(id) => match schema.type_def(*id) {
            TypeDef::Enum(_) => true,
            TypeDef::Record(def) => def.fields.iter().any(|f| {
                f.bit.is_some() || is_variable(schema, &f.ty) || needs_check(schema, &f.ty)
            }),
        },
        _ => true,
    }
}

fn type_ident(name: &str) -> String {
    name.to_upper_camel_case()
}

fn variant_ident(name: &str) -> String {
    name.to_upper_camel_case()
}

fn mod_ident(name: &str) -> String {
    escape(&name.to_snake_case())
}

fn field_ident(name: &str) -> String {
    escape(&name.to_snake_case())
}

fn escape(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
        "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
    ];
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}
