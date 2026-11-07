use std::path::Path;

use super::*;
use crate::backend::MARKER;
use crate::compile;

const EXAMPLE1: &str = include_str!("../../../tests/data/example1.uju");
const EXAMPLE2: &str = include_str!("../../../tests/data/example2.uju");

fn files(sources: &[&str], direction: Option<Direction>) -> Vec<GeneratedFile> {
    let schema = compile(sources)
        .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
        .schema;
    Rust { direction }
        .generate(&schema)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn file<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
    let file = files.iter().find(|file| file.path == Path::new(path));
    &file.unwrap_or_else(|| panic!("no `{path}`")).contents
}

/// Every file, concatenated.
fn generated(sources: &[&str], direction: Option<Direction>) -> String {
    files(sources, direction)
        .into_iter()
        .map(|file| file.contents)
        .collect()
}

#[test]
fn each_namespace_gets_a_module_file() {
    let files = files(&[EXAMPLE1, EXAMPLE2], None);
    let paths: Vec<&Path> = files.iter().map(|file| file.path.as_path()).collect();
    assert_eq!(
        paths,
        [
            Path::new("mod.rs"),
            Path::new("foo/mod.rs"),
            Path::new("foo/bar/mod.rs"),
            Path::new("hello/mod.rs"),
        ]
    );
    assert!(
        files
            .iter()
            .all(|file| file.contents.lines().next().unwrap().contains(MARKER)),
        "stale files are told apart by the marker"
    );
    let root = file(&files, "mod.rs");
    assert!(root.contains("pub mod foo;\n"));
    assert!(root.contains("pub mod hello;\n"));
    assert!(root.contains("pub const FINGERPRINT"));
    assert!(file(&files, "foo/mod.rs").contains("pub mod bar;\n"));
    // Nested types stay inline.
    assert!(file(&files, "foo/bar/mod.rs").contains("pub mod my_response {"));
}

#[test]
fn ids_live_in_the_root() {
    let files = files(&[EXAMPLE1, EXAMPLE2], None);
    let root = file(&files, "mod.rs");
    assert!(root.contains(
        "impl ::uju::net::wire::Inbound for self::foo::bar::MyRequest {\n    \
         const INBOUND_ID: u16 = "
    ));
    assert!(
        files[1..]
            .iter()
            .all(|file| !file.contents.contains("_ID: u16"))
    );
}

#[test]
fn raw_namespaces_name_their_files_plainly() {
    let files = files(&["namespace a.type;\nstruct S {}\n"], None);
    assert!(file(&files, "a/mod.rs").contains("pub mod r#type;\n"));
    assert!(file(&files, "a/type/mod.rs").contains("pub struct S {}"));
}

#[test]
fn a_server_decodes_what_it_receives_and_encodes_what_it_sends() {
    let server = generated(&[EXAMPLE1, EXAMPLE2], Some(Direction::In));
    assert!(
        server.contains("pub struct MyRequestView<'a>"),
        "in: viewed"
    );
    assert!(
        !server.contains("impl __w::Encode for MyRequest "),
        "in: not encoded"
    );
    assert!(
        server.contains("impl __w::Encode for MyResponse "),
        "out: encoded"
    );
    assert!(!server.contains("MyResponseView"), "out: not viewed");
    assert!(server.contains("pub struct PingView"), "bi: both");
    assert!(server.contains("impl __w::Encode for Ping "), "bi: both");
    assert!(server.contains("pub enum InboundMessage<'a>"));
    assert!(!server.contains("OutboundMessage"));

    let client = generated(&[EXAMPLE1, EXAMPLE2], Some(Direction::Out));
    assert!(client.contains("pub enum OutboundMessage<'a>"));
    assert!(client.contains("impl __w::Encode for MyRequest "));
    assert!(!client.contains("InboundMessage"));
}

#[test]
fn keywords_and_view_methods_are_escaped() {
    let contents = generated(&[EXAMPLE1, EXAMPLE2], None);
    assert!(contents.contains("pub r#type: u8,"), "raw field");
    assert!(
        contents.contains("pub fn decode_(&self) -> u16"),
        "accessor kept off `decode`"
    );
    assert!(
        contents.contains("pub decode: u16,"),
        "owned fields keep their name"
    );
}

#[test]
fn shared_message_names_qualify_their_variants() {
    let contents = generated(&[EXAMPLE1, EXAMPLE2], None);
    assert!(contents.contains("FooBarMyRequest(self::foo::bar::MyRequestView<'a>)"));
    assert!(contents.contains("HelloMyRequest(self::hello::MyRequestView<'a>)"));
    assert!(contents.contains("Everything(self::foo::bar::EverythingView<'a>)"));
}

#[test]
fn colliding_names_are_reported() {
    let schema = compile(&["namespace a;\nstruct B {}\nstruct BView {}\n"])
        .unwrap()
        .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(error.message, "`a.B` and `a.BView` both generate `BView`");
}

#[test]
fn nested_types_get_a_module_named_after_their_parent() {
    let contents = generated(&[EXAMPLE1, EXAMPLE2], None);
    assert!(contents.contains("pub mod my_response {"));
    assert!(contents.contains("pub mod status {"));
    assert!(contents.contains("pub mod http_detail {"));
    assert!(contents.contains("pub result: self::my_response::Result,"));
    // `HTTPDetail` lives in `foo::bar::status`.
    assert!(contents.contains("pub kind: self::http_detail::Kind,"));
    assert!(contents.contains("pub color: super::Color,"));
    assert_eq!(snake("MyResponse"), "my_response");
    assert_eq!(snake("HTTPDetail"), "http_detail");
    assert_eq!(snake("Vec2D"), "vec2_d");
    assert_eq!(snake("Type"), "r#type");
}

#[test]
fn qualified_variants_are_pascal_case() {
    let contents = generated(
        &[
            "namespace my_ns;\nmessage[in] M {}\n",
            "namespace other;\nmessage[in] M {}\n",
        ],
        None,
    );
    assert!(contents.contains("MyNsM(&'a self::my_ns::MView)"));
    assert!(contents.contains("OtherM(&'a self::other::MView)"));
}

#[test]
fn colliding_variants_are_reported() {
    let schema = compile(&[
        "namespace aB.c;\nmessage[in] M {}\n",
        "namespace a.bC;\nmessage[in] M {}\n",
    ])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`a.bC.M` and `aB.c.M` both generate `InboundMessage::ABCM`"
    );
    // A peer that never decodes them has no dispatch enum to collide in.
    let generated = Rust {
        direction: Some(Direction::Out),
    }
    .generate(&schema);
    assert!(generated.is_ok());
}

#[test]
fn unsupported_types_are_reported() {
    let schema = compile(&[
        "namespace a;\nmessage[in] M { id: uuid, ids: vec<uuid>, u: option<uentity> }\n",
    ])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`a.M.id`: the Rust backend does not support `uuid` yet\n\
         `a.M.ids`: the Rust backend does not support `uuid` yet\n\
         `a.M.u`: the Rust backend does not support `uentity` yet"
    );
}

#[test]
fn nested_modules_collide_with_namespaces() {
    let schema = compile(&[
        "namespace a;\nstruct Foo { enum E { X } e: E }\n",
        "namespace a.foo;\nstruct Bar {}\n",
    ])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`namespace a.foo` and `declarations nested in a.Foo` both generate `foo`"
    );

    // A const alone opens the module.
    let schema = compile(&[
        "namespace a;\nstruct Foo { const C: u8 = 1; }\n",
        "namespace a.foo;\nstruct Bar {}\n",
    ])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`namespace a.foo` and `declarations nested in a.Foo` both generate `foo`"
    );
}

#[test]
fn consts_are_shouty_beside_their_scope() {
    let files = files(
        &[concat!(
            "namespace a;\n",
            "const MaxHp: u32 = 100;\n",
            "const HTTPPort: u16 = 80;\n",
            "const Low: i64 = -9223372036854775808;\n",
            "const Big: u64 = 18446744073709551615;\n",
            "const Rate: f32 = 0.1;\n",
            "const Whole: f64 = 2;\n",
            "const On: bool = false;\n",
            "const Text: string = \"say \\\"hi\\\"\\n\\u{7}é\";\n",
            "message[in] SetName {\n",
            "    const MaxLength: u8 = 16;\n",
            "    name: string,\n",
            "}\n",
            "struct Limits { const Slots: u8 = 4; }\n",
        )],
        None,
    );
    let contents = file(&files, "a/mod.rs");
    for line in [
        "pub const MAX_HP: u32 = 100;",
        "pub const HTTP_PORT: u16 = 80;",
        "pub const LOW: i64 = -9223372036854775808;",
        "pub const BIG: u64 = 18446744073709551615;",
        "pub const RATE: f32 = 0.1;",
        "pub const WHOLE: f64 = 2.0;",
        "pub const ON: bool = false;",
        r#"pub const TEXT: &str = "say \"hi\"\n\u{7}é";"#,
        "pub mod set_name {\n    pub const MAX_LENGTH: u8 = 16;\n}",
        "pub mod limits {\n    pub const SLOTS: u8 = 4;\n}",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
    // Sorted by qualified name, before the types.
    let at = |needle| contents.find(needle).unwrap();
    assert!(at("BIG") < at("HTTP_PORT") && at("HTTP_PORT") < at("MAX_HP"));
    assert!(at("TEXT") < at("pub struct Limits"));
}

#[test]
fn colliding_consts_are_reported() {
    let schema = compile(&["namespace a;\nconst MaxHp: u8 = 1;\nconst MAXHp: u8 = 2;\n"])
        .unwrap()
        .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`a.MaxHp` and `a.MAXHp` both generate `MAX_HP`"
    );

    // Values and types live apart in Rust.
    files(&["namespace a;\nconst AB: u8 = 1;\nstruct Ab {}\n"], None);

    let schema = compile(&["namespace a;\nconst _: u8 = 1;\nconst _1: u8 = 2;\n"])
        .unwrap()
        .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(error.message, "`a._` has no words to name a Rust constant");
    let schema = compile(&["namespace a;\nconst _1: u8 = 2;\n"])
        .unwrap()
        .schema;
    let files = Rust { direction: None }.generate(&schema).unwrap();
    assert!(file(&files, "a/mod.rs").contains("pub const _1: u8 = 2;"));
}

#[test]
fn output_ignores_source_order() {
    assert_eq!(
        generated(&[EXAMPLE1, EXAMPLE2], None),
        generated(&[EXAMPLE2, EXAMPLE1], None)
    );
}

#[test]
fn types_are_newtypes_read_by_value() {
    let contents = generated(
        &["namespace a;\n\
           type PartyId = u64;\n\
           message[in] M { id: PartyId, ids: vec<PartyId>, owners: map<PartyId, u8>, opt: option<PartyId> }\n"],
        None,
    );
    for line in [
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]\n\
         pub struct PartyId(pub u64);",
        "impl __w::Newtype for PartyId {\n    \
         type Cell = __w::U64;\n\n    \
         fn from_cell(cell: &Self::Cell) -> Self {\n        \
         PartyId(__w::Cell::get(cell))\n    }\n}",
        "pub id: PartyId,",
        "pub ids: ::std::vec::Vec<PartyId>,",
        "pub owners: ::std::collections::HashMap<PartyId, u8>,",
        "{\n                let v = &v.0;\n                w.put(&v.to_le_bytes())?;\n            }",
        "pub fn id(&self) -> PartyId {",
        "__w::read::<__w::Typed<PartyId>>(b, ",
        "pub fn ids(&self) -> __w::Packed<'a, __w::Typed<PartyId>> {",
        "__w::Packed<'a, __w::Entry<__w::Typed<PartyId>, u8>>",
        "pub fn opt(&self) -> ::core::option::Option<PartyId> {",
        "id: self.id(),",
        "ids: self.ids().iter().collect(),",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
    assert!(!contents.contains("PartyIdView"), "a type is its own view");

    // Only a peer that decodes a type reads it through a cell.
    let server = generated(
        &["namespace a;\ntype Id = u64;\nmessage[out] M { id: Id }\n"],
        Some(Direction::In),
    );
    assert!(server.contains("pub struct Id(pub u64);"));
    assert!(!server.contains("Newtype"));
}

#[test]
fn types_over_bool_are_bits() {
    let contents = generated(
        &["namespace a;\n\
           type Flag = bool;\n\
           type Strong = Flag;\n\
           struct S { on: Flag, strong: Strong, maybe: option<Strong>, flags: vec<Flag> }\n"],
        None,
    );
    for line in [
        "if self.on.0 {",
        "if self.strong.0.0 {",
        "if let Some(v) = self.maybe {\n            bits[0] |= 0x04;\n            if v.0.0 {",
        "Flag(__w::bit(b, 0))",
        "Strong(Flag(__w::bit(b, 1)))",
        "Some(Strong(Flag(__w::bit(b, 3))))",
        "pub struct Strong(pub Flag);",
        "type Cell = __w::Typed<Flag>;",
        // In a collection, a byte each.
        "__w::Packed<'a, __w::Typed<Flag>>",
        "__w::de::boolean(b[0], at, \"S.flags\")?;",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
}

#[test]
fn types_derive_what_their_target_supports() {
    let contents = generated(
        &["namespace a;\n\
           type Ratio = f32;\n\
           type Owner = entity;\n\
           enum Color { Red = 1 }\n\
           type Hue = Color;\n\
           struct S { r: Ratio, o: Owner }\n"],
        None,
    );
    for line in [
        "#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]\npub struct Ratio(pub f32);",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\n\
         pub struct Owner(pub __w::Entity);",
        "type Cell = __w::EntityCell;",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]\n\
         pub struct Hue(pub Color);",
        "type Cell = __w::EnumCell<Color>;",
        // A struct holding them derives no more than they do.
        "#[derive(Debug, Clone, Copy, PartialEq)]\npub struct S {",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
}

#[test]
fn times_are_std_times_checked_against_their_range() {
    let contents = generated(
        &["namespace a;\n\
           type At = timestamp;\n\
           union U { A, B(duration) }\n\
           struct S { at: At, t: timestamp, mt: option<timestamp>, ds: vec<duration>, u: U }\n\
           struct D { d: duration }\n"],
        None,
    );
    for line in [
        // `SystemTime` has no `Default`; `Duration` does.
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\n\
         pub struct At(pub ::std::time::SystemTime);",
        "type Cell = __w::TimestampCell;",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub enum U {\n    \
         A,\n    B(::std::time::Duration),\n}",
        "#[derive(Debug, Clone, PartialEq, Eq, Hash)]\npub struct S {\n    \
         pub at: At,\n    pub t: ::std::time::SystemTime,\n    \
         pub mt: ::core::option::Option<::std::time::SystemTime>,\n    \
         pub ds: ::std::vec::Vec<::std::time::Duration>,\n    pub u: U,\n}",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]\npub struct D {",
        // Encoders convert and range check.
        "w.put(&__w::en::timestamp(*v, \"S.at\")?)?;",
        "w.put(&__w::en::timestamp(*v, \"S.t\")?)?;",
        "w.put(&__w::en::timestamp(*v, \"S.mt\")?)?;",
        "w.put(&__w::en::duration(*v, \"S.ds\")?)?;",
        "w.put(&__w::en::duration(*v, \"U.B\")?)?;",
        // Views read them by value, once validated.
        "pub fn t(&self) -> ::std::time::SystemTime {",
        "unsafe { __w::read::<__w::TimestampCell>(b, at) }",
        "pub fn ds(&self) -> __w::Packed<'a, __w::DurationCell> {",
        "Self::B(unsafe { __w::read::<__w::DurationCell>(b, 0) })",
        "__w::de::timestamp(b, at, \"S.at\")?;",
        "__w::de::timestamp(b, at, \"S.t\")?;",
        "__w::de::timestamp(b, at, \"S.mt\")?;",
        "__w::de::duration(b, at, \"S.ds\")?;",
        "__w::de::duration(b, at, \"U.B\")?;",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
}

#[test]
fn type_constraints_hold_wherever_it_is_used() {
    let contents = generated(
        &["namespace a;\n\
           type Level = u8[max=100];\n\
           type Depth = i32[min=-5, max=5];\n\
           struct S { l: Level, ls: vec<Level>, d: Depth }\n"],
        None,
    );
    for line in [
        "__w::en::range(*v, u8::MIN, 100, \"S.l\")?;",
        "__w::en::range(*v, u8::MIN, 100, \"S.ls\")?;",
        "__w::de::range(__w::de::plain::<u8>(b, 0), u8::MIN, 100, at, \"S.l\")?;",
        "__w::de::range(__w::de::plain::<u8>(b, 0), u8::MIN, 100, at, \"S.ls\")?;",
        // Narrowed as the target is.
        "type Cell = __w::Biased<i32, u8, { -5 }>;",
        "w.put(&((*v).wrapping_sub(-5) as u8).to_le_bytes())?;",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
}

#[test]
fn type_names_claim_no_view() {
    files(&["namespace a;\ntype A = u8;\nstruct AView {}\n"], None);
}

#[test]
fn unsupported_types_behind_a_type_or_in_a_union_are_reported() {
    let schema = compile(&["namespace a;\ntype Owner = uentity;\ntype Id = uuid;\n\
         union U { A, B(uuid), C { at: uentity } }\nstruct S { owner: Owner }\n"])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`a.Owner`: the Rust backend does not support `uentity` yet\n\
         `a.Id`: the Rust backend does not support `uuid` yet\n\
         `a.U.B`: the Rust backend does not support `uuid` yet\n\
         `a.U.C.at`: the Rust backend does not support `uentity` yet"
    );
}

const INVENTORY: &str = "namespace a;\n\
    struct Item { id: u32, name: string }\n\
    struct Point { x: f32, y: f32 }\n\
    union InventoryOp {\n\
        Add { item: Item, slot: u8, hot: bool },\n\
        Remove(u8),\n\
        Clear,\n\
        Move { to: Point },\n\
        Rename(string[max_length=8]),\n\
        Yes(bool),\n\
    }\n";

#[test]
fn unions_are_enums_written_after_their_tag() {
    let contents = generated(&[INVENTORY], None);
    for line in [
        // Floats rule out `Eq` and `Hash`, and a union has no `Default`.
        "#[derive(Debug, Clone, PartialEq)]\npub enum InventoryOp {\n    \
         Add {\n        item: Item,\n        slot: u8,\n        hot: bool,\n    },\n    \
         Remove(u8),\n    Clear,\n    Move {\n        to: Point,\n    },\n    \
         Rename(::std::string::String),\n    Yes(bool),\n}",
        "pub const MAX_ENCODED_SIZE: ::core::option::Option<usize> = None;",
        // The tag's byte, then the variant's data.
        "Self::Add { item: __f0, slot: __f1, hot: __f2 } => {\n                \
         let mut size = 3;\n                {\n                    let v = __f0;\n                    \
         size += __w::Encode::encoded_size(v);\n                }\n                size\n            }",
        "Self::Remove(_) => 2,",
        "Self::Clear => 1,",
        "Self::Move { .. } => 9,",
        "Self::Rename(v) => 1 + v.len(),",
        "Self::Yes(_) => 2,",
        "__w::en::encode(self, buf, \"InventoryOp\")",
        "Self::Add { item: __f0, slot: __f1, hot: __f2 } => {\n                \
         w.put(&[1])?;\n                let mut bits = [0u8; 1];\n                \
         if *__f2 {",
        "{\n                    let v = __f1;\n                    w.put(&v.to_le_bytes())?;",
        "Self::Remove(v) => {\n                w.put(&[2])?;\n                \
         w.put(&v.to_le_bytes())?;\n            }",
        "Self::Clear => w.put(&[3])?,",
        "__w::en::length(v.len(), 0, 8, \"InventoryOp.Rename\")?;",
        "Self::Yes(v) => {\n                w.put(&[6])?;\n                \
         w.put(&[*v as u8])?;\n            }",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
    assert!(!contents.contains("Default)]\npub enum InventoryOp"));

    let contents = generated(
        &["namespace a;\nunion U { A, B(u8), C { x: u16 } }\n"],
        None,
    );
    assert!(contents.contains("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub enum U {"));
}

#[test]
fn union_views_match_on_the_tag() {
    let contents = generated(&[INVENTORY], None);
    for line in [
        "#[derive(Debug, Clone, Copy)]\npub enum InventoryOpView<'a> {\n    \
         Add(self::inventory_op::AddView<'a>),\n    Remove(u8),\n    Clear,\n    \
         Move(&'a self::inventory_op::MoveView),\n    Rename(&'a str),\n    Yes(bool),\n}",
        "pub unsafe fn decode_unchecked(buf: &'a [u8]) -> Self {\n        \
         unsafe { <Self as __w::Var<'_>>::read(buf) }\n    }",
        "__w::de::size(buf, \"InventoryOp\")?;\n        \
         <InventoryOpView<'_> as __w::Validate>::validate_at(buf, 0)",
        "Self::Add(v) => InventoryOp::Add {\n                item: v.item().to_owned(),\n                \
         slot: v.slot(),\n                hot: v.hot(),\n            },",
        "Self::Remove(v) => InventoryOp::Remove(v),",
        "Self::Clear => InventoryOp::Clear,",
        "Self::Rename(v) => InventoryOp::Rename(::std::string::String::from(v)),",
        // Validation reads the tag, then checks the variant's data.
        "__w::de::at_least(buf, 1, at, \"InventoryOp\")?;\n        \
         let (b, at) = (&buf[1..], at + 1);\n        \
         match __w::de::plain::<u8>(buf, 0) {\n            \
         1 => {\n                \
         <self::inventory_op::AddView<'_> as __w::Validate>::validate_at(b, at)?;\n            }",
        "2 => {\n                __w::de::exact(b, 1, at, \"InventoryOp.Remove\")?;\n            }",
        "3 => {\n                __w::de::exact(b, 0, at, \"InventoryOp.Clear\")?;\n            }",
        "4 => {\n                \
         <self::inventory_op::MoveView as __w::Validate>::validate_at(b, at)?;\n            }",
        "__w::de::length(b.len(), 0, 8, at, \"InventoryOp.Rename\")?;",
        "6 => {\n                __w::de::exact(b, 1, at, \"InventoryOp.Yes\")?;\n                \
         __w::de::boolean(b[0], at, \"InventoryOp.Yes\")?;\n            }",
        "_ => return __w::de::tag(at - 1, \"InventoryOp\"),",
        // Reading trusts the tag.
        "let b = &bytes[1..];\n        match __w::de::plain::<u8>(bytes, 0) {",
        "1 => Self::Add(unsafe { <self::inventory_op::AddView<'a> as __w::Var<'a>>::read(b) }),",
        "2 => Self::Remove(unsafe { __w::read::<u8>(b, 0) }),",
        "3 => Self::Clear,",
        "4 => Self::Move(unsafe { __w::cast::<self::inventory_op::MoveView>(b) }),",
        "5 => Self::Rename(unsafe { <&'a str as __w::Var<'a>>::read(b) }),",
        "6 => Self::Yes(unsafe { __w::read::<__w::Bool>(b, 0) }),",
        "_ => unsafe { ::core::hint::unreachable_unchecked() },",
        "impl<'a> __w::Decode<'a> for InventoryOpView<'a> {",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
}

#[test]
fn variant_views_live_in_the_unions_module() {
    let contents = generated(&[INVENTORY], None);
    let start = contents
        .find("pub mod inventory_op {")
        .expect("the variants' module");
    let end = start + contents[start..].find("\n}\n").expect("the module's end");
    let module = &contents[start..end];
    for line in [
        "/// [`super::InventoryOp::Add`], read in place.\n    \
         #[derive(Clone, Copy)]\n    pub struct AddView<'a> {",
        "pub fn as_bytes(&self) -> &'a [u8] {",
        "pub fn item(&self) -> super::ItemView<'a> {",
        "impl __w::Validate for AddView<'_> {",
        "__w::de::at_least(buf, 2, at, \"InventoryOp.Add\")?;",
        "impl<'a> __w::Var<'a> for AddView<'a> {",
        "impl ::core::fmt::Debug for AddView<'_> {",
        "pub struct MoveView([u8; 8]);",
        "unsafe impl __w::Cell for MoveView {",
    ] {
        assert!(module.contains(line), "no `{line}` in\n{module}");
    }
    // Only reached through the union's view.
    for method in [
        "fn decode(",
        "fn decode_unchecked(",
        "fn validate(",
        "fn to_owned(",
        "__w::Decode<",
    ] {
        assert!(!module.contains(method), "`{method}` in\n{module}");
    }
    // No module without a variant with fields.
    let contents = generated(&["namespace a;\nunion U { A, B(u8) }\n"], None);
    assert!(!contents.contains("pub mod u {"));
}

#[test]
fn union_views_borrow_only_when_a_variant_does() {
    let contents = generated(
        &["namespace a;\n\
           union Small { On, Level(u8) }\n\
           union Inner { Text(string), Small(Small) }\n\
           struct S { s: Small, i: Inner, maybe: option<Small>, all: vec<Small>, inners: vec<Inner> }\n"],
        None,
    );
    for line in [
        "pub enum SmallView {",
        "impl SmallView {\n    /// Validates `buf` as exactly one encoding.\n    \
         pub fn decode(buf: &[u8]) -> ::core::result::Result<Self, __w::DecodeError> {",
        "impl __w::Validate for SmallView {",
        "impl<'a> __w::Var<'a> for SmallView {",
        "impl<'a> __w::Decode<'a> for SmallView {",
        "pub enum InnerView<'a> {\n    Text(&'a str),\n    Small(SmallView),\n}",
        // Nothing to check in a `string` without constraints, so `b` goes
        // unused.
        "1 => {\n                __w::de::utf8(b, at, \"Inner.Text\")?;",
        "2 => {\n                <SmallView as __w::Validate>::validate_at(b, at)?;",
        "pub fn s(&self) -> SmallView {",
        "unsafe { <SmallView as __w::Var<'a>>::read(&b[range]) }",
        "pub fn i(&self) -> InnerView<'a> {",
        "pub fn maybe(&self) -> ::core::option::Option<SmallView> {",
        "pub fn all(&self) -> __w::Prefixed<'a, SmallView, ",
        "pub fn inners(&self) -> __w::Prefixed<'a, InnerView<'a>, ",
        "<SmallView as __w::Validate>::validate_at(b, at)?;",
        "<InnerView<'_> as __w::Validate>::validate_at(b, at)?;",
        "maybe: self.maybe().map(|v| v.to_owned()),",
        "inners: self.inners().iter().map(|v| v.to_owned()).collect(),",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }

    // Without data to check, `b` is not bound.
    let contents = generated(&["namespace a;\nunion U { A(bytes) }\n"], None);
    assert!(contents.contains("match __w::de::plain::<u8>(buf, 0) {\n            1 => {}"));
    assert!(contents.contains("_ => return __w::de::tag(at, \"U\"),"));
    assert!(!contents.contains("let (b, at) = (&buf[1..], at + 1);"));
    // Without data, nothing is read past the tag.
    let contents = generated(&["namespace a;\nunion U { A, B }\n"], None);
    assert!(!contents.contains("let b = &bytes[1..];"));
}

#[test]
fn unions_of_many_variants_take_a_u16_tag() {
    let variants: Vec<String> = (0..300).map(|i| format!("V{i}")).collect();
    let source = format!(
        "namespace a;\nunion U {{ {}, Last(u8) }}\n",
        variants.join(", ")
    );
    let contents = generated(&[&source], None);
    for line in [
        "Self::V0 => 2,",
        "Self::Last(_) => 3,",
        "Self::V0 => w.put(&1u16.to_le_bytes())?,",
        "w.put(&301u16.to_le_bytes())?;",
        "__w::de::at_least(buf, 2, at, \"U\")?;",
        "match __w::de::plain::<__w::U16>(buf, 0) {",
        "match __w::de::plain::<__w::U16>(bytes, 0) {",
        "301 => Self::Last(unsafe { __w::read::<u8>(b, 0) }),",
        "_ => return __w::de::tag(at - 2, \"U\"),",
    ] {
        assert!(contents.contains(line), "no `{line}` in\n{contents}");
    }
}

#[test]
fn unions_follow_the_peers_roles() {
    let source = "namespace a;\n\
        union In { A { x: u8 }, B }\n\
        union Out { A { x: u8 }, B }\n\
        message[in] Req { u: In }\n\
        message[out] Resp { u: Out }\n";
    let server = generated(&[source], Some(Direction::In));
    assert!(server.contains("pub enum InView<'a> {"));
    assert!(server.contains("pub mod r#in {"));
    assert!(!server.contains("impl __w::Encode for In {"));
    assert!(server.contains("impl __w::Encode for Out {"));
    assert!(!server.contains("OutView"));
    assert!(!server.contains("pub mod out {"));
}

#[test]
fn variant_modules_collide_with_namespaces() {
    let schema = compile(&[
        "namespace a;\nunion Foo { A { x: u8 } }\n",
        "namespace a.foo;\nstruct Bar {}\n",
    ])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`namespace a.foo` and `the variant views of a.Foo` both generate `foo`"
    );
    // Without fields, no module.
    files(
        &[
            "namespace a;\nunion Foo { A(u8) }\n",
            "namespace a.foo;\nstruct Bar {}\n",
        ],
        None,
    );
    // The view is claimed too.
    let schema = compile(&["namespace a;\nunion U { A }\nstruct UView {}\n"])
        .unwrap()
        .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(error.message, "`a.U` and `a.UView` both generate `UView`");
}
