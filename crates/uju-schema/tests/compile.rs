use uju_schema::compile;
use uju_schema::ir::*;

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
    let diags = compile(src).expect_err("expected compile error");
    diags
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn sample1_layouts() {
    let schema = compile(include_str!("sample1.uju")).unwrap();
    assert_eq!(schema.namespace, vec!["foo", "bar"]);

    let empty = record(&schema, "EmptyStruct");
    assert_eq!(empty.layout.size, Size::Fixed(0));

    let optional = record(&schema, "Optional");
    assert_eq!(optional.layout.bitmap_bytes, 1);
    assert_eq!(optional.layout.size, Size::Fixed(5));
    assert_eq!(optional.fields[0].offset, 1);
    assert_eq!(optional.fields[0].bit, Some(0));
    assert!(optional.fields[0].optional);

    let position = record(&schema, "Position");
    assert_eq!(position.kind, RecordKind::Component);
    assert_eq!(position.layout.size, Size::Fixed(8));
    assert_eq!(position.fields[0].offset, 0);
    assert_eq!(position.fields[1].offset, 4);

    let scalars = record(&schema, "Scalars");
    assert_eq!(scalars.layout.size, Size::Fixed(79));
    let offsets: Vec<u32> = scalars.fields.iter().map(|f| f.offset).collect();
    assert_eq!(
        offsets,
        vec![0, 1, 3, 7, 15, 16, 18, 22, 30, 31, 35, 43, 51, 59, 67]
    );

    let non_scalars = record(&schema, "NonScalars");
    assert_eq!(non_scalars.layout.size, Size::Variable);
    assert_eq!(non_scalars.layout.fixed_size, 24);
    assert_eq!(non_scalars.layout.bitmap_bytes, 0);
    let offsets: Vec<u32> = non_scalars.fields.iter().map(|f| f.offset).collect();
    assert_eq!(offsets, (0..12).map(|i| i * 2).collect::<Vec<u32>>());
}

#[test]
fn sample1_enums_consts_messages() {
    let schema = compile(include_str!("sample1.uju")).unwrap();

    let color = enum_def(&schema, "Color");
    assert_eq!(color.repr, Prim::U8);
    let values: Vec<u64> = color.variants.iter().map(|v| v.value).collect();
    assert_eq!(values, vec![0, 1, 5]);

    let numbers = enum_def(&schema, "Numbers");
    assert_eq!(numbers.repr, Prim::U32);

    assert_eq!(schema.consts[0].value, ConstValue::Int(1));
    assert_eq!(
        schema.consts[1].value,
        ConstValue::Variant {
            ty: schema.type_id("Color").unwrap(),
            index: 0,
        }
    );

    let ids: Vec<u32> = ["Scalars", "NonScalars", "MyRequest", "MyResponse"]
        .iter()
        .map(|name| record(&schema, name).message.unwrap().id)
        .collect();
    assert_eq!(ids, vec![0, 1, 2, 3]);

    let request = record(&schema, "MyRequest");
    assert_eq!(
        request.message.unwrap().returns,
        Some(schema.type_id("MyResponse").unwrap())
    );

    let result_id = schema.type_id("MyResponse.Result").unwrap();
    let response = record(&schema, "MyResponse");
    assert_eq!(response.fields[0].ty, Ty::Ref(result_id));
    assert_eq!(response.message.unwrap().returns, None);
}

#[test]
fn vec_recursion_is_allowed() {
    let schema = compile("message Node { children: vec<Node>, tag: u8, }").unwrap();
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
