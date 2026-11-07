//! Emits the compiled IR as JSON (`ujuc protocol ir`), with the wire layout
//! of every declaration, for generators written outside `ujuc`. Each
//! namespace gets `namespaces/<name>.json`; the fingerprint and the message
//! id order, which change with any message, live in `schema.json`.
//! `docs/protocol/ir.md` describes the format.

use crate::backend::{self, Backend, Error, GeneratedFile};
use crate::ir::wire::{Int, Layout, Sequence, Size, Slot, UnionLayout, Width, Wire};
use crate::ir::{
    self, ConstId, ConstValue, Constraints, DefKind, Field, NamespaceId, Number, Type, TypeId,
    TypeKind, VariantData,
};

/// The file holding what spans namespaces. Namespace files sit in their own
/// directory, so no namespace name can collide with it.
const ROOT: &str = "schema.json";
/// The directory namespace files are emitted into.
const NAMESPACES: &str = "namespaces";

pub struct Ir;

impl Backend for Ir {
    fn generate(&self, schema: &ir::Schema) -> Result<Vec<GeneratedFile>, Error> {
        let wire = backend::wire(schema)?;
        let mut files = vec![GeneratedFile::new(ROOT, root(schema, &wire))];
        files.extend((0..schema.namespaces.len()).map(|index| {
            let id = NamespaceId(index);
            let name = schema.namespace(id).name.join(".");
            let contents = namespace(schema, &wire, id, &name);
            GeneratedFile::new(format!("{NAMESPACES}/{name}.json"), contents)
        }));
        Ok(files)
    }
}

fn root(schema: &ir::Schema, wire: &Wire) -> String {
    let mut json = Json::new();
    json.begin('{');
    json.entry_num("fingerprint", wire.fingerprint());

    json.key("namespaces");
    json.begin('[');
    for namespace in &schema.namespaces {
        json.item();
        json.quoted(&namespace.name.join("."));
    }
    json.end(']');

    // Index `i` holds the message with id `i + 1`.
    for (key, messages) in [("inbound", wire.inbound()), ("outbound", wire.outbound())] {
        json.key(key);
        json.begin('[');
        for id in messages {
            json.item();
            json.quoted(&schema.qualified(*id));
        }
        json.end(']');
    }

    json.end('}');
    json.finish()
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
    for (index, item) in schema.consts.iter().enumerate() {
        if item.namespace == id {
            json.item();
            const_json(&mut json, schema, ConstId(index));
        }
    }
    json.end(']');

    json.end('}');
    json.finish()
}

fn def_json(json: &mut Json, schema: &ir::Schema, wire: &Wire, id: TypeId) {
    let def = schema.def(id);
    json.begin('{');
    json.entry("name", &def.name);
    json.entry("path", &schema.qualified(id));
    match def.parent {
        Some(parent) => json.entry("parent", &schema.qualified(parent)),
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
            object_json(json, schema, wire, id, &item.fields);
        }
        DefKind::Message(item) => {
            json.entry("kind", "message");
            json.entry("direction", item.direction.name());
            let ids = wire.ids(id);
            json.key("ids");
            json.begin('{');
            json.entry_opt_num("inbound", ids.inbound);
            json.entry_opt_num("outbound", ids.outbound);
            json.end('}');
            object_json(json, schema, wire, id, &item.fields);
        }
        DefKind::Union(item) => {
            json.entry("kind", "union");
            let layout = wire.union(id);
            json.key("size");
            size_json(json, layout.size);
            json.entry_width("tag", layout.tag);
            json.key("variants");
            json.begin('[');
            for (index, variant) in item.variants.iter().enumerate() {
                json.item();
                json.begin('{');
                json.entry("name", &variant.name);
                json.entry_num("tag", UnionLayout::tag_of(index));
                match &variant.data {
                    VariantData::Unit => json.entry("kind", "unit"),
                    VariantData::Type(ty) => {
                        json.entry("kind", "type");
                        json.key("type");
                        type_json(json, schema, wire, ty);
                    }
                    VariantData::Fields(fields) => {
                        json.entry("kind", "fields");
                        let layout = layout.variants[index]
                            .as_ref()
                            .expect("a variant with fields has a layout");
                        fields_json(json, schema, wire, layout, fields);
                    }
                }
                json.end('}');
            }
            json.end(']');
        }
        DefKind::Type(item) => {
            json.entry("kind", "type");
            json.key("target");
            type_json(json, schema, wire, &item.target);
        }
    }

    json.end('}');
}

fn const_json(json: &mut Json, schema: &ir::Schema, id: ConstId) {
    let item = schema.constant(id);
    json.begin('{');
    json.entry("name", &item.name);
    json.entry("path", &schema.const_qualified(id));
    match item.parent {
        Some(parent) => json.entry("parent", &schema.qualified(parent)),
        None => json.entry_null("parent"),
    }
    json.entry("type", kind_name(&item.ty));
    match &item.value {
        ConstValue::Int(value) => json.entry_num("value", value),
        // `{:?}` keeps the fraction of whole floats: `1.0`, not `1`.
        ConstValue::Float(value) => json.entry_num("value", format!("{value:?}")),
        ConstValue::Bool(value) => json.entry_bool("value", *value),
        ConstValue::String(value) => json.entry("value", value),
        // Microseconds, since the Unix epoch for a timestamp.
        ConstValue::Duration(micros) | ConstValue::Timestamp(micros) => {
            json.entry_num("value", micros)
        }
    }
    json.end('}');
}

/// The layout and fields of a struct or message.
fn object_json(json: &mut Json, schema: &ir::Schema, wire: &Wire, id: TypeId, fields: &[Field]) {
    fields_json(json, schema, wire, wire.layout(id), fields);
}

/// The layout and fields of an object: a struct, a message, or a union's
/// variant.
fn fields_json(
    json: &mut Json,
    schema: &ir::Schema,
    wire: &Wire,
    layout: &Layout,
    fields: &[Field],
) {
    json.key("layout");
    layout_json(json, layout);

    json.key("fields");
    json.begin('[');
    for (field, slot) in fields.iter().zip(&layout.fields) {
        json.item();
        json.begin('{');
        json.entry("name", &field.name);
        json.key("type");
        type_json(json, schema, wire, &field.ty);
        json.entry_bool("optional", field.optional);
        json.key("slot");
        slot_json(json, *slot);
        json.end('}');
    }
    json.end(']');
}

fn layout_json(json: &mut Json, layout: &Layout) {
    json.begin('{');
    json.key("size");
    size_json(json, layout.size);
    json.entry_num("bitmap", layout.bitmap);
    json.entry_num("boundaries", layout.boundaries);
    json.entry_width("boundary", layout.boundary);
    json.entry_num("head", layout.head);
    json.entry_num("vars", layout.vars);
    json.end('}');
}

fn slot_json(json: &mut Json, slot: Slot) {
    json.begin('{');
    match slot {
        Slot::Bit(bit) => {
            json.entry("kind", "bit");
            json.entry_num("bit", bit);
        }
        Slot::At(offset) => {
            json.entry("kind", "at");
            json.entry_num("offset", offset);
        }
        Slot::OptionalBit { presence, value } => {
            json.entry("kind", "optional_bit");
            json.entry_num("presence", presence);
            json.entry_num("value", value);
        }
        Slot::Optional { presence } => {
            json.entry("kind", "optional");
            json.entry_num("presence", presence);
        }
        Slot::Var { slot, presence } => {
            json.entry("kind", "var");
            json.entry_num("slot", slot);
            json.entry_opt_num("presence", presence);
        }
    }
    json.end('}');
}

/// A type as a tree: its kind, what it holds, its constraints, and how it
/// sits on the wire.
fn type_json(json: &mut Json, schema: &ir::Schema, wire: &Wire, ty: &Type) {
    json.begin('{');
    json.entry("kind", kind_name(&ty.kind));
    match &ty.kind {
        TypeKind::Array(item, length) => {
            json.key("item");
            type_json(json, schema, wire, item);
            json.entry_num("length", length);
        }
        TypeKind::Vec(item) | TypeKind::Set(item) => {
            json.key("item");
            type_json(json, schema, wire, item);
        }
        TypeKind::Map(key, value) => {
            json.key("key");
            type_json(json, schema, wire, key);
            json.key("value");
            type_json(json, schema, wire, value);
        }
        TypeKind::Named(id) => json.entry("path", &schema.qualified(*id)),
        _ => {}
    }
    json.key("constraints");
    constraints_json(json, &ty.constraints);

    json.key("wire");
    json.begin('{');
    json.key("size");
    size_json(json, wire.size(ty));
    if let Some(int) = Int::of(ty) {
        json.key("int");
        json.begin('{');
        match int {
            Int::Native(width) => {
                json.entry("kind", "native");
                json.entry_width("width", width);
            }
            Int::Biased { min, width } => {
                json.entry("kind", "biased");
                json.entry_num("min", min);
                json.entry_width("width", width);
            }
        }
        json.end('}');
    }
    if let Some(sequence) = wire.sequence(ty) {
        json.key("sequence");
        json.begin('{');
        match sequence {
            Sequence::Packed { item } => {
                json.entry("kind", "packed");
                json.entry_num("item", item);
            }
            Sequence::Counted { count } => {
                json.entry("kind", "counted");
                json.entry_width("count", count);
            }
            Sequence::Prefixed { count, prefix } => {
                json.entry("kind", "prefixed");
                json.entry_width("count", count);
                json.entry_width("prefix", prefix);
            }
        }
        json.end('}');
    }
    json.end('}');

    json.end('}');
}

fn kind_name(kind: &TypeKind) -> &'static str {
    match kind {
        TypeKind::I8 => "i8",
        TypeKind::I16 => "i16",
        TypeKind::I32 => "i32",
        TypeKind::I64 => "i64",
        TypeKind::U8 => "u8",
        TypeKind::U16 => "u16",
        TypeKind::U32 => "u32",
        TypeKind::U64 => "u64",
        TypeKind::F32 => "f32",
        TypeKind::F64 => "f64",
        TypeKind::Bool => "bool",
        TypeKind::Timestamp => "timestamp",
        TypeKind::Duration => "duration",
        TypeKind::Entity => "entity",
        TypeKind::UEntity => "uentity",
        TypeKind::Uuid => "uuid",
        TypeKind::String => "string",
        TypeKind::Bytes => "bytes",
        TypeKind::Array(..) => "array",
        TypeKind::Vec(_) => "vec",
        TypeKind::Map(..) => "map",
        TypeKind::Set(_) => "set",
        TypeKind::Named(_) => "named",
    }
}

/// Only the constraints that are set; `infinite` and `nan` when true.
fn constraints_json(json: &mut Json, constraints: &Constraints) {
    json.begin('{');
    let number = |json: &mut Json, key: &str, value: Option<Number>| match value {
        // `{:?}` keeps the fraction of whole floats: `1.0`, not `1`.
        Some(Number::Float(value)) => json.entry_num(key, format!("{value:?}")),
        Some(Number::Int(value)) => json.entry_num(key, value),
        None => {}
    };
    number(json, "min", constraints.min);
    number(json, "max", constraints.max);
    if let Some(min_length) = constraints.min_length {
        json.entry_num("min_length", min_length);
    }
    if let Some(max_length) = constraints.max_length {
        json.entry_num("max_length", max_length);
    }
    if constraints.infinite {
        json.entry_bool("infinite", true);
    }
    if constraints.nan {
        json.entry_bool("nan", true);
    }
    json.end('}');
}

fn size_json(json: &mut Json, size: Size) {
    json.begin('{');
    match size {
        Size::Fixed(bytes) => {
            json.entry("kind", "fixed");
            json.entry_num("bytes", bytes);
        }
        Size::Var { max } => {
            json.entry("kind", "var");
            json.entry_opt_num("max", max);
        }
    }
    json.end('}');
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

    fn entry_opt_num(&mut self, key: &str, value: Option<impl std::fmt::Display>) {
        match value {
            Some(value) => self.entry_num(key, value),
            None => self.entry_null(key),
        }
    }

    /// A width, in bytes.
    fn entry_width(&mut self, key: &str, width: Width) {
        self.entry_num(key, width.bytes());
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
    use serde_json::{Value, json};

    use super::*;
    use crate::compile;

    const EXAMPLE1: &str = include_str!("../../tests/data/example1.uju");
    const EXAMPLE2: &str = include_str!("../../tests/data/example2.uju");

    fn generated(sources: &[&str]) -> Vec<GeneratedFile> {
        let schema = compile(sources)
            .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
            .schema;
        Ir.generate(&schema)
            .expect("a lowered schema always lays out")
    }

    fn parse(file: &GeneratedFile) -> Value {
        serde_json::from_str(&file.contents)
            .unwrap_or_else(|error| panic!("`{}`: {error}\n{}", file.path.display(), file.contents))
    }

    /// The file at `path`, parsed.
    fn file(files: &[GeneratedFile], path: &str) -> Value {
        let file = files
            .iter()
            .find(|file| file.path.to_str() == Some(path))
            .unwrap_or_else(|| panic!("no file `{path}`"));
        parse(file)
    }

    /// The declaration named `path` in a single-namespace schema.
    fn def(sources: &[&str], path: &str) -> Value {
        let files = generated(sources);
        assert_eq!(files.len(), 2, "expected the root and a single namespace");
        let namespace = parse(&files[1]);
        namespace["defs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|def| def["path"] == path)
            .unwrap_or_else(|| panic!("no declaration `{path}`"))
            .clone()
    }

    fn field(def: &Value, name: &str) -> Value {
        def["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("no field `{name}`"))
            .clone()
    }

    #[test]
    fn a_root_and_one_file_per_namespace() {
        let files = generated(&[EXAMPLE1, EXAMPLE2]);
        let paths: Vec<_> = files
            .iter()
            .map(|file| file.path.display().to_string())
            .collect();
        assert_eq!(
            paths,
            [
                "schema.json",
                "namespaces/foo.bar.json",
                "namespaces/hello.json"
            ]
        );
        for file in &files {
            parse(file);
        }
    }

    #[test]
    fn the_root_holds_the_fingerprint_and_id_order() {
        let schema = compile(&[EXAMPLE1, EXAMPLE2]).unwrap().schema;
        let wire = Wire::new(&schema).unwrap();
        let files = Ir.generate(&schema).unwrap();
        let root = file(&files, "schema.json");
        assert_eq!(root["fingerprint"], wire.fingerprint());
        assert_eq!(root["namespaces"], json!(["foo.bar", "hello"]));
        let names = |ids: &[TypeId]| -> Vec<String> {
            ids.iter().map(|id| schema.qualified(*id)).collect()
        };
        assert_eq!(root["inbound"], json!(names(wire.inbound())));
        assert_eq!(root["outbound"], json!(names(wire.outbound())));
    }

    #[test]
    fn a_namespace_named_like_the_root_does_not_collide() {
        let files = generated(&["namespace schema\nstruct S {}\n"]);
        assert_eq!(files[0].path.to_str(), Some("schema.json"));
        assert_eq!(files[1].path.to_str(), Some("namespaces/schema.json"));
    }

    #[test]
    fn nested_declarations_name_their_parent() {
        let files = generated(&[EXAMPLE1]);
        let namespace = file(&files, "namespaces/foo.bar.json");
        let result = namespace["defs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|def| def["path"] == "foo.bar.MyResponse.Result")
            .expect("the nested enum");
        assert_eq!(result["name"], "Result");
        assert_eq!(result["parent"], "foo.bar.MyResponse");
    }

    #[test]
    fn consts_carry_their_type_and_value() {
        let files = generated(&[concat!(
            "namespace a\n",
            "const Count: u64 = 18446744073709551615\n",
            "const Low: i8 = -128\n",
            "const Whole: f32 = 2\n",
            "const On: bool = true\n",
            "const Text: string = \"say \\\"hi\\\"\\n\"\n",
            "const Delay: duration = 30s\n",
            "const Start: timestamp = 2026-11-07T12:00:00Z\n",
            "message[in] M { const Inner: u16 = 7 }\n",
        )]);
        let namespace = file(&files, "namespaces/a.json");
        assert_eq!(
            namespace["consts"],
            json!([
                {"name": "Count", "path": "a.Count", "parent": null, "type": "u64", "value": u64::MAX},
                {"name": "Low", "path": "a.Low", "parent": null, "type": "i8", "value": -128},
                {"name": "Whole", "path": "a.Whole", "parent": null, "type": "f32", "value": 2.0},
                {"name": "On", "path": "a.On", "parent": null, "type": "bool", "value": true},
                {"name": "Text", "path": "a.Text", "parent": null, "type": "string", "value": "say \"hi\"\n"},
                {"name": "Delay", "path": "a.Delay", "parent": null, "type": "duration", "value": 30_000_000},
                {"name": "Start", "path": "a.Start", "parent": null, "type": "timestamp", "value": 1_794_052_800_000_000u64},
                {"name": "Inner", "path": "a.M.Inner", "parent": "a.M", "type": "u16", "value": 7},
            ])
        );
        assert!(files[1].contents.contains(r#""value": 2.0"#));
        assert!(files[1].contents.contains(r#""value": 30000000"#));
        assert!(files[1].contents.contains(r#""value": 1794052800000000"#));
        assert!(
            namespace["defs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|def| def["kind"] != "const")
        );
    }

    #[test]
    fn messages_carry_their_direction_and_ids() {
        let sources = [concat!(
            "namespace a\n",
            "message[in] A {}\n",
            "message[out] B {}\n",
            "message[bi] C {}\n",
        )];
        let ids = |name: &str| {
            let def = def(&sources, &format!("a.{name}"));
            (def["direction"].clone(), def["ids"].clone())
        };
        assert_eq!(
            ids("A"),
            (json!("in"), json!({"inbound": 1, "outbound": null}))
        );
        assert_eq!(
            ids("B"),
            (json!("out"), json!({"inbound": null, "outbound": 1}))
        );
        assert_eq!(
            ids("C"),
            (json!("bi"), json!({"inbound": 2, "outbound": 2}))
        );
    }

    #[test]
    fn struct_fields_are_flagged_optional_too() {
        let def = def(
            &["namespace a\nstruct S { o: option<string>, v: u8 }\n"],
            "a.S",
        );
        assert_eq!(field(&def, "o")["optional"], true);
        assert_eq!(field(&def, "o")["type"]["kind"], "string");
        assert_eq!(field(&def, "v")["optional"], false);
    }

    #[test]
    fn objects_carry_their_layout_and_slots() {
        let def = def(
            &[concat!(
                "namespace a\n",
                "struct S {\n",
                "    flag: bool,\n",
                "    maybe: option<bool>,\n",
                "    x: u16,\n",
                "    hp: option<u32>,\n",
                "    name: string,\n",
                "    nick: option<string>,\n",
                "}\n",
            )],
            "a.S",
        );
        assert_eq!(
            def["layout"],
            json!({
                "size": {"kind": "var", "max": null},
                "bitmap": 1,
                "boundaries": 3,
                "boundary": 2,
                "head": 5,
                "vars": 2,
            })
        );
        let slot = |name: &str| field(&def, name)["slot"].clone();
        assert_eq!(slot("flag"), json!({"kind": "bit", "bit": 0}));
        assert_eq!(
            slot("maybe"),
            json!({"kind": "optional_bit", "presence": 1, "value": 2})
        );
        assert_eq!(slot("x"), json!({"kind": "at", "offset": 1}));
        assert_eq!(slot("hp"), json!({"kind": "optional", "presence": 3}));
        assert_eq!(
            slot("name"),
            json!({"kind": "var", "slot": 0, "presence": null})
        );
        assert_eq!(
            slot("nick"),
            json!({"kind": "var", "slot": 1, "presence": 4})
        );
    }

    #[test]
    fn types_are_trees_with_qualified_names() {
        let def = def(
            &[concat!(
                "namespace foo.bar\n",
                "struct Position { x: f32 }\n",
                "message[in] M { m: map<u8, vec<Position>>, a: array<i32, 4> }\n",
            )],
            "foo.bar.M",
        );
        let m = &field(&def, "m")["type"];
        assert_eq!(m["kind"], "map");
        assert_eq!(m["key"]["kind"], "u8");
        assert_eq!(m["value"]["kind"], "vec");
        assert_eq!(m["value"]["item"]["kind"], "named");
        assert_eq!(m["value"]["item"]["path"], "foo.bar.Position");

        let a = &field(&def, "a")["type"];
        assert_eq!(a["kind"], "array");
        assert_eq!(a["length"], 4);
        assert_eq!(a["item"]["kind"], "i32");
        assert_eq!(a["wire"]["size"], json!({"kind": "fixed", "bytes": 16}));
    }

    #[test]
    fn constraints_hold_only_what_is_set() {
        let def = def(
            &[concat!(
                "namespace a\n",
                "message[in] M {\n",
                "    v: vec<string[max_length=14]>[max_length=10, min_length=1],\n",
                "    f: f64[nan, infinite, max=1],\n",
                "    u: u8,\n",
                "}\n",
            )],
            "a.M",
        );
        let v = &field(&def, "v")["type"];
        assert_eq!(v["constraints"], json!({"min_length": 1, "max_length": 10}));
        assert_eq!(v["item"]["constraints"], json!({"max_length": 14}));
        assert_eq!(
            field(&def, "f")["type"]["constraints"],
            json!({"max": 1.0, "infinite": true, "nan": true})
        );
        assert_eq!(field(&def, "u")["type"]["constraints"], json!({}));
        // Whole floats keep their fraction.
        assert!(def.to_string().contains(r#""max":1.0"#), "{def}");
    }

    #[test]
    fn integers_carry_their_encoding() {
        let def = def(
            &["namespace a\nstruct S { n: i32, b: i32[min=-100, max=100] }\n"],
            "a.S",
        );
        assert_eq!(
            field(&def, "n")["type"]["wire"]["int"],
            json!({"kind": "native", "width": 4})
        );
        let b = &field(&def, "b")["type"]["wire"];
        assert_eq!(b["int"], json!({"kind": "biased", "min": -100, "width": 1}));
        assert_eq!(b["size"], json!({"kind": "fixed", "bytes": 1}));
    }

    #[test]
    fn collections_carry_their_sequence() {
        let def = def(
            &[concat!(
                "namespace a\n",
                "struct E {}\n",
                "struct S {\n",
                "    packed: vec<u32>,\n",
                "    counted: vec<E>[max_length=9],\n",
                "    prefixed: vec<string[max_length=16]>[max_length=14],\n",
                "}\n",
            )],
            "a.S",
        );
        let sequence = |name: &str| field(&def, name)["type"]["wire"]["sequence"].clone();
        assert_eq!(sequence("packed"), json!({"kind": "packed", "item": 4}));
        assert_eq!(sequence("counted"), json!({"kind": "counted", "count": 1}));
        assert_eq!(
            sequence("prefixed"),
            json!({"kind": "prefixed", "count": 1, "prefix": 1})
        );
        assert_eq!(
            field(&def, "prefixed")["type"]["wire"]["size"],
            json!({"kind": "var", "max": 238})
        );
    }

    #[test]
    fn unions_carry_their_tag_and_variants() {
        let def = def(
            &[concat!(
                "namespace a\n",
                "type Flag = bool\n",
                "union Op { Add { slot: u8, hot: Flag }, Remove(u8[max=9]), Clear }\n",
            )],
            "a.Op",
        );
        assert_eq!(def["kind"], "union");
        assert_eq!(def["size"], json!({"kind": "var", "max": 3}));
        assert_eq!(def["tag"], 1);
        let variants = def["variants"].as_array().unwrap();
        assert_eq!(variants.len(), 3);

        let add = &variants[0];
        assert_eq!(add["name"], "Add");
        assert_eq!(add["tag"], 1);
        assert_eq!(add["kind"], "fields");
        assert_eq!(
            add["layout"],
            json!({
                "size": {"kind": "fixed", "bytes": 2},
                "bitmap": 1,
                "boundaries": 2,
                "boundary": 1,
                "head": 2,
                "vars": 0,
            })
        );
        assert_eq!(
            field(add, "slot")["slot"],
            json!({"kind": "at", "offset": 1})
        );
        assert_eq!(field(add, "hot")["slot"], json!({"kind": "bit", "bit": 0}));
        assert_eq!(field(add, "hot")["type"]["path"], "a.Flag");

        assert_eq!(
            variants[1],
            json!({
                "name": "Remove",
                "tag": 2,
                "kind": "type",
                "type": {
                    "kind": "u8",
                    "constraints": {"max": 9},
                    "wire": {
                        "size": {"kind": "fixed", "bytes": 1},
                        "int": {"kind": "native", "width": 1},
                    },
                },
            })
        );
        assert_eq!(
            variants[2],
            json!({"name": "Clear", "tag": 3, "kind": "unit"})
        );
    }

    #[test]
    fn wide_unions_take_a_two_byte_tag() {
        let variants: Vec<String> = (0..256).map(|index| format!("V{index}")).collect();
        let def = def(
            &[&format!(
                "namespace a\nunion U {{ {} }}\n",
                variants.join(", ")
            )],
            "a.U",
        );
        assert_eq!(def["tag"], 2);
        assert_eq!(def["size"], json!({"kind": "var", "max": 2}));
        assert_eq!(def["variants"][255]["tag"], 256);
    }

    #[test]
    fn types_carry_their_target() {
        let sources = [concat!(
            "namespace a\n",
            "enum E: u16 { A }\n",
            "type Level = i32[min=-100, max=100]\n",
            "type Hue = E\n",
            "type Rank = Level\n",
            "struct S { level: Level }\n",
        )];
        let level = def(&sources, "a.Level");
        assert_eq!(level["kind"], "type");
        assert_eq!(
            level["target"],
            json!({
                "kind": "i32",
                "constraints": {"min": -100, "max": 100},
                "wire": {
                    "size": {"kind": "fixed", "bytes": 1},
                    "int": {"kind": "biased", "min": -100, "width": 1},
                },
            })
        );
        let hue = def(&sources, "a.Hue");
        assert_eq!(hue["target"]["kind"], "named");
        assert_eq!(hue["target"]["path"], "a.E");
        assert_eq!(
            hue["target"]["wire"]["size"],
            json!({"kind": "fixed", "bytes": 2})
        );
        assert_eq!(def(&sources, "a.Rank")["target"]["path"], "a.Level");

        // A field names the type, and takes its size, not its `int`.
        let level = &field(&def(&sources, "a.S"), "level")["type"];
        assert_eq!(level["kind"], "named");
        assert_eq!(level["path"], "a.Level");
        assert_eq!(
            level["wire"],
            json!({"size": {"kind": "fixed", "bytes": 1}})
        );
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        let files = generated(&["namespace a\nstruct S {}\n"]);
        assert!(
            files[1].contents.contains(r#""fields": []"#)
                && files[1].contents.contains(r#""consts": []"#),
            "{}",
            files[1].contents
        );
    }
}
