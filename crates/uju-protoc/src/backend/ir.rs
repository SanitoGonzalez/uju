//! Emits the compiled IR as JSON, one file per namespace: every name fully
//! qualified and every type spelled the way it is written in source.

use crate::backend::{Backend, Error, GeneratedFile};
use crate::ir::{self, DefKind, NamespaceId, Type, TypeId, TypeKind};

pub struct Ir;

impl Backend for Ir {
    fn generate(&self, schema: &ir::Schema) -> Result<Vec<GeneratedFile>, Error> {
        Ok((0..schema.namespaces.len())
            .map(|index| {
                let id = NamespaceId(index);
                let name = schema.namespace(id).name.join(".");
                let contents = namespace(schema, id, &name);
                GeneratedFile::new(format!("{name}.json"), contents)
            })
            .collect())
    }
}

fn namespace(schema: &ir::Schema, id: NamespaceId, name: &str) -> String {
    let mut json = Json::new();
    json.begin('{');
    json.entry("namespace", name);

    json.key("defs");
    json.begin('[');
    for (index, def) in schema.defs.iter().enumerate() {
        if def.namespace == id {
            json.item();
            def_json(&mut json, schema, TypeId(index));
        }
    }
    json.end(']');

    json.end('}');
    json.finish()
}

fn def_json(json: &mut Json, schema: &ir::Schema, id: TypeId) {
    let def = schema.def(id);
    json.begin('{');
    json.entry("name", &def.name);
    json.entry("path", &qualified(schema, id));
    match def.parent {
        Some(parent) => json.entry("parent", &qualified(schema, parent)),
        None => json.entry_null("parent"),
    }

    match &def.kind {
        DefKind::Enum(item) => {
            json.entry("kind", "enum");
            json.entry("repr", item.repr.name());
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
        DefKind::Struct(item) => {
            json.entry("kind", "struct");
            json.key("fields");
            json.begin('[');
            for field in &item.fields {
                json.item();
                json.begin('{');
                json.entry("name", &field.name);
                json.entry("type", &type_name(schema, &field.ty));
                json.end('}');
            }
            json.end(']');
        }
        DefKind::Message(item) => {
            json.entry("kind", "message");
            json.entry("direction", item.direction.name());
            json.key("fields");
            json.begin('[');
            for field in &item.fields {
                json.item();
                json.begin('{');
                json.entry("name", &field.name);
                json.entry("type", &type_name(schema, &field.ty));
                json.entry_bool("optional", field.optional);
                json.end('}');
            }
            json.end(']');
        }
    }

    json.end('}');
}

/// The type as it would be written in source, with named types qualified
/// and constraints in canonical order: `vec<foo.bar.Position>[max_length=8]`.
fn type_name(schema: &ir::Schema, ty: &Type) -> String {
    let name = match &ty.kind {
        TypeKind::I8 => "i8".to_string(),
        TypeKind::I16 => "i16".to_string(),
        TypeKind::I32 => "i32".to_string(),
        TypeKind::I64 => "i64".to_string(),
        TypeKind::U8 => "u8".to_string(),
        TypeKind::U16 => "u16".to_string(),
        TypeKind::U32 => "u32".to_string(),
        TypeKind::U64 => "u64".to_string(),
        TypeKind::F32 => "f32".to_string(),
        TypeKind::F64 => "f64".to_string(),
        TypeKind::Bool => "bool".to_string(),
        TypeKind::Timestamp => "timestamp".to_string(),
        TypeKind::Interval => "interval".to_string(),
        TypeKind::Entity => "entity".to_string(),
        TypeKind::UEntity => "uentity".to_string(),
        TypeKind::Uuid => "uuid".to_string(),
        TypeKind::String => "string".to_string(),
        TypeKind::Bytes => "bytes".to_string(),
        TypeKind::Array(item, len) => format!("array<{}, {len}>", type_name(schema, item)),
        TypeKind::Vec(item) => format!("vec<{}>", type_name(schema, item)),
        TypeKind::Set(item) => format!("set<{}>", type_name(schema, item)),
        TypeKind::Map(key, value) => {
            format!(
                "map<{}, {}>",
                type_name(schema, key),
                type_name(schema, value)
            )
        }
        TypeKind::Named(id) => qualified(schema, *id),
    };
    format!("{name}{}", ty.constraints)
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
    fn declarations_carry_qualified_names() {
        let contents = only(&["namespace a.b;\nstruct S { x: u8, y: u32 }\n"]);
        assert!(contents.contains(r#""path": "a.b.S""#), "{contents}");
        assert!(contents.contains(r#""name": "x""#), "{contents}");
        assert!(contents.contains(r#""type": "u32""#), "{contents}");
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
    }

    #[test]
    fn messages_carry_their_direction() {
        let contents = only(&[SAMPLE1]);
        for direction in ["in", "out", "bi"] {
            assert!(
                contents.contains(&format!(r#""direction": "{direction}""#)),
                "{contents}"
            );
        }
    }

    #[test]
    fn optional_fields_are_flagged() {
        let contents = only(&["namespace a;\nmessage[in] M { o: option<string> }\n"]);
        assert!(contents.contains(r#""type": "string""#), "{contents}");
        assert!(contents.contains(r#""optional": true"#), "{contents}");
    }

    #[test]
    fn types_are_spelled_out_and_qualified() {
        let contents = only(&[concat!(
            "namespace foo.bar;\n",
            "struct Position { x: f32 }\n",
            "message[in] M { m: map<u8, vec<Position>>, a: array<i32, 4> }\n",
        )]);
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
    fn constraints_follow_their_type() {
        let contents = only(&[concat!(
            "namespace a;\n",
            "message[in] M {\n",
            "    v: vec<string[max_length=14]>[max_length=10, min_length=1],\n",
            "    f: f64[nan, infinite, max=1],\n",
            "}\n",
        )]);
        assert!(
            contents
                .contains(r#""type": "vec<string[max_length=14]>[min_length=1, max_length=10]""#),
            "{contents}"
        );
        assert!(
            contents.contains(r#""type": "f64[max=1.0, infinite, nan]""#),
            "{contents}"
        );
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        let contents = only(&["namespace a;\nstruct S {}\n"]);
        assert!(contents.contains(r#""fields": []"#), "{contents}");
    }
}
