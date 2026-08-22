//! Emits the compiled IR as JSON, one file per namespace: every name fully
//! qualified, every type spelled the way it is written in source, and the
//! wire layout folded in beside the declaration it belongs to.

use crate::backend::{Backend, Error, GeneratedFile};
use crate::ir::wire::{DefWire, Wire};
use crate::ir::{self, DefKind, NamespaceId, Type, TypeId, Value};

pub struct Ir;

impl Backend for Ir {
    fn generate(&self, schema: &ir::Schema) -> Result<Vec<GeneratedFile>, Error> {
        let wire = Wire::new(schema);
        Ok((0..schema.namespaces.len())
            .map(|index| {
                let id = NamespaceId(index);
                let name = schema.namespace(id).name.join(".");
                let contents = namespace(schema, &wire, id, &name);
                GeneratedFile::new(format!("{name}.json"), contents)
            })
            .collect())
    }
}

fn namespace(schema: &ir::Schema, wire: &Wire, id: NamespaceId, name: &str) -> String {
    let mut json = Json::new();
    json.begin('{');
    json.entry("namespace", name);

    json.key("defs");
    json.begin('[');
    for (index, def) in schema.defs.iter().enumerate() {
        if def.namespace == id {
            json.item();
            def_json(&mut json, schema, wire, TypeId(index));
        }
    }
    json.end(']');

    json.key("consts");
    json.begin('[');
    for konst in schema.consts.iter().filter(|konst| konst.namespace == id) {
        json.item();
        const_json(&mut json, schema, konst);
    }
    json.end(']');

    json.end('}');
    json.finish()
}

fn def_json(json: &mut Json, schema: &ir::Schema, wire: &Wire, id: TypeId) {
    let def = schema.def(id);
    json.begin('{');
    json.entry("name", &def.name);
    json.entry("path", &qualified(schema, id));
    match def.parent {
        Some(parent) => json.entry("parent", &qualified(schema, parent)),
        None => json.entry_null("parent"),
    }

    match (&def.kind, wire.def(id)) {
        (DefKind::Enum(item), DefWire::Enum(layout)) => {
            json.entry("kind", "enum");
            json.entry("repr", item.repr.name());
            json.entry_num("size", layout.size);
            json.entry_num("align", layout.align);
            json.key("variants");
            json.begin('[');
            for variant in &item.variants {
                json.item();
                json.begin('{');
                json.entry("name", &variant.name);
                json.entry_num("value", variant.value);
                json.end('}');
            }
            json.end(']');
        }
        (DefKind::Struct(item), DefWire::Struct(item_wire)) => {
            json.entry("kind", "struct");
            json.entry_num("size", item_wire.layout.size);
            json.entry_num("align", item_wire.layout.align);
            json.key("fields");
            json.begin('[');
            for (field, offset) in item.fields.iter().zip(&item_wire.offsets) {
                json.item();
                json.begin('{');
                json.entry("name", &field.name);
                json.entry("type", &type_name(schema, &field.ty));
                json.entry_num("offset", offset);
                json.end('}');
            }
            json.end(']');
        }
        (DefKind::Union(item), DefWire::Union(item_wire)) => {
            json.entry("kind", "union");
            json.entry_num("payload_align", item_wire.payload_align);
            json.key("members");
            json.begin('[');
            for (tag, member) in item.members.iter().enumerate() {
                json.item();
                json.begin('{');
                json.entry("name", &member.name);
                json.entry("type", &type_name(schema, &member.ty));
                json.entry_num("tag", tag);
                json.end('}');
            }
            json.end(']');
        }
        (DefKind::Message(item), DefWire::Message(item_wire)) => {
            json.entry("kind", "message");
            match item.response {
                Some(response) => json.entry("response", &qualified(schema, response)),
                None => json.entry_null("response"),
            }
            json.entry_num("align", item_wire.align);
            json.entry_num("fixed_size", item_wire.fixed_size);
            json.entry_num("presence_offset", item_wire.presence_offset);
            json.entry_num("presence_bytes", item_wire.presence_bytes);
            json.key("fields");
            json.begin('[');
            for (field, field_wire) in item.fields.iter().zip(&item_wire.fields) {
                json.item();
                json.begin('{');
                json.entry("name", &field.name);
                json.entry("type", &type_name(schema, &field.ty));
                json.entry_bool("optional", field.optional);
                json.entry_bool("fixed", wire.is_fixed(&field.ty));
                json.entry_num("offset", field_wire.offset);
                match field_wire.presence {
                    Some(bit) => json.entry_num("presence", bit),
                    None => json.entry_null("presence"),
                }
                json.end('}');
            }
            json.end(']');
        }
        _ => unreachable!("the wire layout is parallel to the declarations"),
    }

    json.end('}');
}

fn const_json(json: &mut Json, schema: &ir::Schema, konst: &ir::Const) {
    json.begin('{');
    json.entry("name", &konst.name);
    json.entry("path", &qualified_const(schema, konst));
    match konst.parent {
        Some(parent) => json.entry("parent", &qualified(schema, parent)),
        None => json.entry_null("parent"),
    }
    json.entry("type", &type_name(schema, &konst.ty));
    json.key("value");
    value_json(json, schema, &konst.value);
    json.end('}');
}

fn value_json(json: &mut Json, schema: &ir::Schema, value: &Value) {
    match value {
        Value::Bool(value) => json.raw(if *value { "true" } else { "false" }),
        Value::Int(value) => json.raw(&value.to_string()),
        // Infinities and NaN have no JSON spelling; fall back to a string.
        Value::Float(value) if value.is_finite() => json.raw(&format!("{value:?}")),
        Value::Float(value) => json.quoted(&value.to_string()),
        Value::Str(value) => json.quoted(value),
        Value::EnumVariant(id, index) => {
            let DefKind::Enum(item) = &schema.def(*id).kind else {
                unreachable!("an enum variant constant names an enum");
            };
            json.quoted(&format!(
                "{}.{}",
                qualified(schema, *id),
                item.variants[*index].name
            ));
        }
    }
}

/// The type as it would be written in source, with named types qualified:
/// `map<u32, vec<foo.bar.Position>>`.
fn type_name(schema: &ir::Schema, ty: &Type) -> String {
    match ty {
        Type::I8 => "i8".to_string(),
        Type::I16 => "i16".to_string(),
        Type::I32 => "i32".to_string(),
        Type::I64 => "i64".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Bool => "bool".to_string(),
        Type::Timestamp => "timestamp".to_string(),
        Type::Interval => "interval".to_string(),
        Type::Entity => "entity".to_string(),
        Type::UEntity => "uentity".to_string(),
        Type::Uuid => "uuid".to_string(),
        Type::String => "string".to_string(),
        Type::Bytes => "bytes".to_string(),
        Type::Array(item, len) => format!("array<{}, {len}>", type_name(schema, item)),
        Type::Vec(item) => format!("vec<{}>", type_name(schema, item)),
        Type::Set(item) => format!("set<{}>", type_name(schema, item)),
        Type::Map(key, value) => {
            format!(
                "map<{}, {}>",
                type_name(schema, key),
                type_name(schema, value)
            )
        }
        Type::Named(id) => qualified(schema, *id),
    }
}

/// A declaration's namespace, the declarations it is nested in, and its own
/// name, joined with `.`.
fn qualified(schema: &ir::Schema, id: TypeId) -> String {
    let mut name = schema.namespace(schema.def(id).namespace).name.join(".");
    for segment in schema.path(id) {
        name.push('.');
        name.push_str(segment);
    }
    name
}

fn qualified_const(schema: &ir::Schema, konst: &ir::Const) -> String {
    let mut name = match konst.parent {
        Some(parent) => qualified(schema, parent),
        None => schema.namespace(konst.namespace).name.join("."),
    };
    name.push('.');
    name.push_str(&konst.name);
    name
}

/// Just enough of a JSON writer for the shapes above, tracking commas and
/// indentation so the output is stable and diffable.
struct Json {
    out: String,
    /// Whether the next value is the first in its container, innermost last.
    first: Vec<bool>,
}

impl Json {
    fn new() -> Json {
        Json {
            out: String::new(),
            first: Vec::new(),
        }
    }

    fn finish(mut self) -> String {
        self.out.push('\n');
        self.out
    }

    fn begin(&mut self, open: char) {
        self.out.push(open);
        self.first.push(true);
    }

    fn end(&mut self, close: char) {
        let empty = self.first.pop().expect("`end` follows a `begin`");
        if !empty {
            self.out.push('\n');
            self.indent();
        }
        self.out.push(close);
    }

    /// Opens the next member of the current container.
    fn item(&mut self) {
        let first = self.first.last_mut().expect("inside a container");
        if !*first {
            self.out.push(',');
        }
        *first = false;
        self.out.push('\n');
        self.indent();
    }

    fn key(&mut self, key: &str) {
        self.item();
        self.quoted(key);
        self.out.push_str(": ");
    }

    fn entry(&mut self, key: &str, value: &str) {
        self.key(key);
        self.quoted(value);
    }

    fn entry_num(&mut self, key: &str, value: impl std::fmt::Display) {
        self.key(key);
        self.out.push_str(&value.to_string());
    }

    fn entry_bool(&mut self, key: &str, value: bool) {
        self.key(key);
        self.raw(if value { "true" } else { "false" });
    }

    fn entry_null(&mut self, key: &str) {
        self.key(key);
        self.raw("null");
    }

    fn raw(&mut self, value: &str) {
        self.out.push_str(value);
    }

    fn quoted(&mut self, value: &str) {
        self.out.push('"');
        for ch in value.chars() {
            match ch {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\n' => self.out.push_str("\\n"),
                '\r' => self.out.push_str("\\r"),
                '\t' => self.out.push_str("\\t"),
                ch if ch < ' ' => self.out.push_str(&format!("\\u{:04x}", ch as u32)),
                ch => self.out.push(ch),
            }
        }
        self.out.push('"');
    }

    fn indent(&mut self) {
        for _ in 0..self.first.len() {
            self.out.push_str("  ");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    const SAMPLE1: &str = include_str!("../../tests/data/sample1.uju");
    const SAMPLE2: &str = include_str!("../../tests/data/sample2.uju");

    fn generated(sources: &[&str]) -> Vec<GeneratedFile> {
        let schema =
            compile(sources).unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"));
        Ir.generate(&schema).expect("the IR always emits")
    }

    fn only(sources: &[&str]) -> String {
        let files = generated(sources);
        assert_eq!(files.len(), 1, "expected a single namespace");
        files.into_iter().next().unwrap().contents
    }

    #[test]
    fn one_file_per_namespace() {
        let files = generated(&[SAMPLE1, SAMPLE2]);
        let paths: Vec<_> = files
            .iter()
            .map(|file| file.path.display().to_string())
            .collect();
        assert_eq!(paths, ["foo.bar.json", "hello.json"]);
    }

    #[test]
    fn declarations_carry_qualified_names_and_layout() {
        let contents = only(&["namespace a.b;\nstruct S { x: u8, y: u32 }\n"]);
        assert!(contents.contains(r#""path": "a.b.S""#), "{contents}");
        assert!(contents.contains(r#""align": 4"#), "{contents}");
        assert!(
            contents.contains(r#""name": "x""#) && contents.contains(r#""offset": 4"#),
            "{contents}"
        );
    }

    #[test]
    fn nested_declarations_name_their_parent() {
        let contents = only(&[SAMPLE1]);
        assert!(
            contents.contains(r#""path": "foo.bar.MyResponse.Result""#),
            "{contents}"
        );
        assert!(
            contents.contains(r#""parent": "foo.bar.MyResponse""#),
            "{contents}"
        );
        assert!(
            contents.contains(r#""response": "foo.bar.MyResponse""#),
            "{contents}"
        );
    }

    #[test]
    fn types_are_spelled_out_and_qualified() {
        let contents = only(&[SAMPLE1]);
        assert!(
            contents.contains(r#""type": "map<u8, vec<foo.bar.Position>>""#),
            "{contents}"
        );
        assert!(
            contents.contains(r#""type": "array<i32, 4>""#),
            "{contents}"
        );
    }

    #[test]
    fn constants_carry_their_evaluated_value() {
        let contents = only(&[SAMPLE1]);
        assert!(contents.contains(r#""value": 1"#), "{contents}");
        assert!(
            contents.contains(r#""value": "foo.bar.Color.Red""#),
            "{contents}"
        );
    }

    #[test]
    fn strings_are_escaped() {
        let contents = only(&["namespace a;\nconst Q: string = \"a\\\"b\\n\";\n"]);
        assert!(contents.contains(r#""value": "a\"b\n""#), "{contents}");
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        let contents = only(&["namespace a;\nstruct S {}\n"]);
        assert!(contents.contains(r#""fields": []"#), "{contents}");
        assert!(contents.contains(r#""consts": []"#), "{contents}");
    }
}
