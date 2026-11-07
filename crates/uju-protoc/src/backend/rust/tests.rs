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
        "namespace a;\nmessage[in] M { t: timestamp, ids: vec<uuid>, u: option<uentity> }\n",
    ])
    .unwrap()
    .schema;
    let error = Rust { direction: None }.generate(&schema).unwrap_err();
    assert_eq!(
        error.message,
        "`a.M.t`: the Rust backend does not support `timestamp` yet\n\
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
