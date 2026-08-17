use uju_schema::ir::*;
use uju_schema::{Source, compile, compile_one};

const SAMPLE1: &str = include_str!("sample1.uju");
const SAMPLE2: &str = include_str!("sample2.uju");

fn sample1() -> Schema {
    compile_one(SAMPLE1).unwrap()
}

fn record<'a>(schema: &'a Schema, name: &str) -> &'a RecordDef {
    match schema.type_def(schema.type_id(name).unwrap()) {
        TypeDef::Record(def) => def,
        _ => panic!("`{name}` is not a record"),
    }
}

fn enum_def<'a>(schema: &'a Schema, name: &str) -> &'a EnumDef {
    match schema.type_def(schema.type_id(name).unwrap()) {
        TypeDef::Enum(def) => def,
        _ => panic!("`{name}` is not an enum"),
    }
}

fn errors(src: &str) -> String {
    let diags = compile_one(src).expect_err("expected compile error");
    diags
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn sample1_layouts() {
    let schema = sample1();

    let empty = record(&schema, "foo.bar.EmptyStruct");
    assert_eq!(empty.layout.size, Size::Fixed(0));

    let optional = record(&schema, "foo.bar.Optional");
    assert_eq!(optional.layout.bitmap_bytes, 1);
    assert_eq!(optional.layout.size, Size::Fixed(5));
    assert_eq!(optional.fields[0].offset, 1);
    assert_eq!(optional.fields[0].bit, Some(0));
    assert!(optional.fields[0].optional);

    let position = record(&schema, "foo.bar.Position");
    assert_eq!(position.kind, RecordKind::Component);
    assert_eq!(position.layout.size, Size::Fixed(8));
    assert_eq!(position.fields[0].offset, 0);
    assert_eq!(position.fields[1].offset, 4);

    let scalars = record(&schema, "foo.bar.Scalars");
    assert_eq!(scalars.layout.size, Size::Fixed(79));
    let offsets: Vec<u32> = scalars.fields.iter().map(|f| f.offset).collect();
    assert_eq!(
        offsets,
        vec![0, 1, 3, 7, 15, 16, 18, 22, 30, 31, 35, 43, 51, 59, 67]
    );

    let non_scalars = record(&schema, "foo.bar.NonScalars");
    assert_eq!(non_scalars.layout.size, Size::Variable);
    assert_eq!(non_scalars.layout.fixed_size, 24);
    assert_eq!(non_scalars.layout.bitmap_bytes, 0);
    let offsets: Vec<u32> = non_scalars.fields.iter().map(|f| f.offset).collect();
    assert_eq!(offsets, (0..12).map(|i| i * 2).collect::<Vec<u32>>());
}

#[test]
fn sample1_enums_consts_messages() {
    let schema = sample1();

    let color = enum_def(&schema, "foo.bar.Color");
    assert_eq!(color.repr, Prim::U8);
    let values: Vec<u64> = color.variants.iter().map(|v| v.value).collect();
    assert_eq!(values, vec![0, 1, 5]);

    assert_eq!(enum_def(&schema, "foo.bar.Numbers").repr, Prim::U32);

    assert_eq!(schema.consts[0].value, ConstValue::Int(1));
    assert_eq!(
        schema.consts[1].value,
        ConstValue::Variant {
            ty: schema.type_id("foo.bar.Color").unwrap(),
            index: 0,
        }
    );

    let ids: Vec<u32> = ["Scalars", "NonScalars", "MyRequest", "MyResponse"]
        .iter()
        .map(|name| {
            record(&schema, &format!("foo.bar.{name}"))
                .message
                .unwrap()
                .id
        })
        .collect();
    assert_eq!(ids, vec![0, 1, 2, 3]);

    let request = record(&schema, "foo.bar.MyRequest");
    assert_eq!(
        request.message.unwrap().returns,
        Some(schema.type_id("foo.bar.MyResponse").unwrap())
    );

    let result_id = schema.type_id("foo.bar.MyResponse.Result").unwrap();
    let response = record(&schema, "foo.bar.MyResponse");
    assert_eq!(response.fields[0].ty, Ty::Ref(result_id));
    assert_eq!(response.message.unwrap().returns, None);
}

#[test]
fn multi_file_resolution() {
    let schema = compile(&[
        Source::new("sample1.uju", SAMPLE1),
        Source::new("sample2.uju", SAMPLE2),
    ])
    .unwrap();

    let external = record(&schema, "hello.ExternalTypes");
    assert_eq!(external.name.namespace, vec!["hello"]);
    assert_eq!(
        external.fields[0].ty,
        Ty::Ref(schema.type_id("foo.bar.Velocity").unwrap()),
        "unqualified name resolves through `use foo.bar`"
    );
    assert_eq!(
        external.fields[1].ty,
        Ty::Ref(schema.type_id("foo.bar.Position").unwrap()),
        "fully qualified name resolves directly"
    );

    let mut namespaces = schema.namespaces();
    namespaces.sort();
    assert_eq!(
        namespaces,
        vec![
            vec!["foo".to_string(), "bar".to_string()],
            vec!["hello".to_string()]
        ]
    );
}

#[test]
fn sample2_alone_fails() {
    let diags = compile(&[Source::new("sample2.uju", SAMPLE2)]).unwrap_err();
    let text = diags
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("unknown namespace"), "{text}");
}

#[test]
fn diagnostics_carry_file_index() {
    let diags = compile(&[
        Source::new("a.uju", "namespace a;\nstruct Ok { x: i32, }"),
        Source::new("b.uju", "namespace b;\nstruct Bad { x: Missing, }"),
    ])
    .unwrap_err();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file, 1);
    assert!(diags[0].message.contains("unknown type"));
}

#[test]
fn schema_hash_is_stable_and_sensitive() {
    let a = compile_one("struct S { x: i32, }").unwrap();
    let b = compile_one("struct S { x: i32, }").unwrap();
    let c = compile_one("struct S { x: u32, }").unwrap();
    let d = compile_one("struct S { y: i32, }").unwrap();
    assert_eq!(a.hash(), b.hash());
    assert_ne!(a.hash(), c.hash());
    assert_ne!(a.hash(), d.hash());
}

#[test]
fn vec_recursion_is_allowed() {
    let schema = compile_one("message Node { children: vec<Node>, tag: u8, }").unwrap();
    let node = record(&schema, "Node");
    assert_eq!(node.layout.size, Size::Variable);
    assert_eq!(node.layout.fixed_size, 3);
}

#[test]
fn compile_errors() {
    assert!(errors("struct S { s: string, }").contains("must be fixed-size"));
    assert!(errors("enum E: i8 { A, }").contains("unsigned"));
    assert!(errors("enum E: u8 { A = 256, }").contains("does not fit"));
    assert!(errors("enum E { A, B = 0, }").contains("duplicate enum value"));
    assert!(errors("message M { x: Unknown, }").contains("unknown type"));
    assert!(errors("struct A { b: B, }\nstruct B { a: A, }").contains("recursive"));
    assert!(errors("message M { x: set<vec<i32>>, }").contains("cannot be containers"));
    assert!(errors("message M { x: map<vec<i32>, u8>, }").contains("cannot be containers"));
    assert!(errors("message M { x: i32, x: u8, }").contains("duplicate field"));
    assert!(errors("struct A {}\nstruct A {}").contains("duplicate definition"));
    assert!(errors("const C: u8 = 300;").contains("out of range"));
    assert!(errors("const C: i32 = true;").contains("does not match"));
    assert!(errors("const C: bytes = 1;").contains("const type"));
    assert!(errors("message A -> B {}\nstruct B {}").contains("is not a message"));
}
