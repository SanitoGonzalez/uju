use uju_schema::ast::*;
use uju_schema::{lexer, parser};

fn parse(src: &str) -> Schema {
    let tokens = lexer::lex(src).expect("lex failed");
    parser::parse(&tokens).unwrap_or_else(|errors| panic!("parse failed: {errors:#?}"))
}

fn path_str(path: &Path) -> String {
    path.0
        .iter()
        .map(|i| i.node.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn ty_str(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Prim(p) => p.name().to_string(),
        TypeRef::Named(p) => path_str(p),
        TypeRef::Vec(t) => format!("vec<{}>", ty_str(&t.node)),
        TypeRef::Set(t) => format!("set<{}>", ty_str(&t.node)),
        TypeRef::Map(k, v) => format!("map<{}, {}>", ty_str(&k.node), ty_str(&v.node)),
        TypeRef::Optional(t) => format!("{}?", ty_str(&t.node)),
    }
}

fn field_tys(fields: &[Field]) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|f| (f.name.node.clone(), ty_str(&f.ty.node)))
        .collect()
}

#[test]
fn sample1() {
    let schema = parse(include_str!("sample1.uju"));

    assert_eq!(path_str(schema.namespace.as_ref().unwrap()), "foo.bar");
    assert!(schema.uses.is_empty());
    assert_eq!(schema.items.len(), 12);

    let Item::Const(number_one) = &schema.items[0] else {
        panic!()
    };
    assert_eq!(number_one.name.node, "NumberOne");
    assert_eq!(ty_str(&number_one.ty.node), "i32");
    assert_eq!(number_one.value.node, Expr::Int(1));

    let Item::Enum(color) = &schema.items[1] else {
        panic!()
    };
    assert_eq!(color.name.node, "Color");
    assert_eq!(color.repr.as_ref().unwrap().node, Prim::U8);
    let values: Vec<_> = color
        .variants
        .iter()
        .map(|v| (v.name.node.as_str(), v.value.as_ref().map(|s| s.node)))
        .collect();
    assert_eq!(
        values,
        vec![("Red", None), ("Green", None), ("Blue", Some(5))]
    );

    let Item::Enum(numbers) = &schema.items[2] else {
        panic!()
    };
    assert_eq!(numbers.repr, None);
    assert_eq!(numbers.variants[2].value.as_ref().unwrap().node, 1000000000);

    let Item::Const(favorite) = &schema.items[3] else {
        panic!()
    };
    assert_eq!(ty_str(&favorite.ty.node), "Color");
    let Expr::Path(value) = &favorite.value.node else {
        panic!()
    };
    assert_eq!(path_str(value), "Color.Red");

    let Item::Struct(empty) = &schema.items[4] else {
        panic!()
    };
    assert_eq!(empty.kind, StructKind::Struct);
    assert!(empty.fields.is_empty());

    let Item::Struct(optional) = &schema.items[5] else {
        panic!()
    };
    assert_eq!(
        field_tys(&optional.fields),
        vec![("opt".into(), "i32?".into())]
    );

    let Item::Struct(position) = &schema.items[6] else {
        panic!()
    };
    assert_eq!(position.kind, StructKind::Component);
    assert_eq!(
        field_tys(&position.fields),
        vec![("x".into(), "f32".into()), ("y".into(), "f32".into())]
    );

    let Item::Message(scalars) = &schema.items[8] else {
        panic!()
    };
    assert_eq!(scalars.name.node, "Scalars");
    assert_eq!(scalars.returns, None);
    assert_eq!(scalars.fields.len(), 15);
    assert_eq!(ty_str(&scalars.fields[11].ty.node), "timestamp");
    assert_eq!(ty_str(&scalars.fields[12].ty.node), "interval");
    assert_eq!(ty_str(&scalars.fields[13].ty.node), "entity");
    assert_eq!(ty_str(&scalars.fields[14].ty.node), "uentity");

    let Item::Message(non_scalars) = &schema.items[9] else {
        panic!()
    };
    assert_eq!(
        field_tys(&non_scalars.fields),
        vec![
            ("v".into(), "vec<i32>".into()),
            ("s".into(), "set<i32>".into()),
            ("m".into(), "map<i32, i32>".into()),
            ("v1".into(), "vec<Position>".into()),
            ("v2".into(), "vec<Velocity>".into()),
            ("v2d".into(), "vec<vec<Position>>".into()),
            ("vs".into(), "vec<set<i32>>".into()),
            ("s1".into(), "set<Position>".into()),
            ("m1".into(), "map<u32, Position>".into()),
            ("m2".into(), "map<u8, vec<Position>>".into()),
            ("str".into(), "string".into()),
            ("b".into(), "bytes".into()),
        ]
    );

    let Item::Message(request) = &schema.items[10] else {
        panic!()
    };
    assert_eq!(path_str(request.returns.as_ref().unwrap()), "MyResponse");

    let Item::Message(response) = &schema.items[11] else {
        panic!()
    };
    assert_eq!(response.items.len(), 1);
    let Item::Enum(result) = &response.items[0] else {
        panic!()
    };
    assert_eq!(result.name.node, "Result");
    assert_eq!(
        field_tys(&response.fields),
        vec![("result".into(), "Result".into())]
    );
}

#[test]
fn sample2() {
    let schema = parse(include_str!("sample2.uju"));

    assert_eq!(path_str(schema.namespace.as_ref().unwrap()), "hello");
    assert_eq!(schema.uses.len(), 1);
    assert_eq!(path_str(&schema.uses[0]), "foo.bar");

    let Item::Message(message) = &schema.items[0] else {
        panic!()
    };
    assert_eq!(
        field_tys(&message.fields),
        vec![
            ("velocity".into(), "Velocity".into()),
            ("position".into(), "foo.bar.Position".into()),
        ]
    );
}

#[test]
fn lex_error() {
    assert!(lexer::lex("struct Foo { x: i32, } @").is_err());
}

#[test]
fn parse_error() {
    let tokens = lexer::lex("struct Foo { x: }").unwrap();
    assert!(parser::parse(&tokens).is_err());
}

#[test]
fn no_optional_in_containers() {
    let tokens = lexer::lex("struct Foo { x: vec<i32?>, }").unwrap();
    assert!(parser::parse(&tokens).is_err());
    let tokens = lexer::lex("struct Foo { x: map<i8, i32?>, }").unwrap();
    assert!(parser::parse(&tokens).is_err());
}

#[test]
fn empty_source() {
    let schema = parse("");
    assert_eq!(schema.namespace, None);
    assert!(schema.items.is_empty());
}
