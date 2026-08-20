pub mod ast;

use std::fmt;

use chumsky::error::Rich;
use chumsky::input::{Input, ValueInput};
use chumsky::primitive::{any, choice, end, just};
use chumsky::recovery::{skip_then_retry_until, skip_until};
use chumsky::recursive::recursive;
use chumsky::{IterParser, Parser, extra, select};

use crate::lexer::{Span, Spanned, Token};
use crate::parser::ast::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

/// Parse the tokens of `src`, returning the schema if one could be built along
/// with every syntax error that was found.
pub fn parse<'src>(
    src: &'src str,
    tokens: &[Spanned<Token<'src>>],
) -> (Option<Schema<'src>>, Vec<ParseError>) {
    let eoi = src.len()..src.len();
    let (schema, errors) = schema()
        .parse(tokens.split_token_span(eoi))
        .into_output_errors();

    let errors = errors
        .into_iter()
        .map(|error| ParseError {
            span: error.span().clone(),
            message: error.reason().to_string(),
        })
        .collect();

    (schema, errors)
}

type Extra<'tok, 'src> = extra::Err<Rich<'tok, Token<'src>, Span>>;

fn schema<'tok, 'src: 'tok, I>() -> impl Parser<'tok, I, Schema<'src>, Extra<'tok, 'src>>
where
    I: ValueInput<'tok, Token = Token<'src>, Span = Span>,
{
    let ident = select! { Token::Ident(name) = e => (name, e.span()) }.labelled("identifier");

    let path = ident
        .separated_by(just(Token::Dot))
        .at_least(1)
        .collect::<Vec<_>>()
        .map_with(|segments, e| Path {
            segments,
            span: e.span(),
        })
        .labelled("path");

    let ty = recursive(|ty| {
        let len = select! { Token::Int(value) => value }.try_map(|value, span: Span| {
            u32::try_from(value)
                .map(|len| (len, span.clone()))
                .map_err(|_| Rich::custom(span, format!("`{value}` is not a valid array length")))
        });

        let arg = ty.clone().delimited_by(just(Token::Lt), just(Token::Gt));

        let vec_ty = just(Token::Ident("vec"))
            .ignore_then(arg.clone())
            .map(|item| TypeKind::Vec(Box::new(item)));

        let set_ty = just(Token::Ident("set"))
            .ignore_then(arg)
            .map(|item| TypeKind::Set(Box::new(item)));

        let map_ty = just(Token::Ident("map"))
            .ignore_then(
                ty.clone()
                    .then_ignore(just(Token::Comma))
                    .then(ty.clone())
                    .delimited_by(just(Token::Lt), just(Token::Gt)),
            )
            .map(|(key, value)| TypeKind::Map(Box::new(key), Box::new(value)));

        let array_ty = just(Token::Ident("array"))
            .ignore_then(
                ty.then_ignore(just(Token::Comma))
                    .then(len)
                    .delimited_by(just(Token::Lt), just(Token::Gt)),
            )
            .map(|(item, len)| TypeKind::Array(Box::new(item), len));

        let prim_ty = ident.try_map(|(name, span), _: Span| match Prim::from_name(name) {
            Some(prim) => Ok(TypeKind::Prim(prim)),
            None => Err(Rich::custom(
                span,
                format!("`{name}` is not a builtin type"),
            )),
        });

        let named_ty = path
            .clone()
            .try_map(|path, span: Span| match path.segments[..] {
                [(name, _)] if matches!(name, "vec" | "set" | "map" | "array") => Err(
                    Rich::custom(span, format!("`{name}` requires type arguments")),
                ),
                _ => Ok(TypeKind::Named(path)),
            });

        choice((vec_ty, set_ty, map_ty, array_ty, prim_ty, named_ty))
            .map_with(|kind, e| Type {
                kind,
                span: e.span(),
            })
            .labelled("type")
            .boxed()
    });

    let field = ident
        .then_ignore(just(Token::Colon))
        .then(ty.clone())
        .then(just(Token::Question).or_not())
        .map(|((name, ty), question)| Field {
            name,
            ty,
            optional: question.is_some(),
        })
        .boxed();

    let value = select! {
        Token::Bool(value) => Value::Bool(value),
        Token::Int(value) => Value::Int(value),
        Token::Float(value) => Value::Float(value),
        Token::Str(value) => Value::Str(value),
    }
    .or(path.clone().map(Value::Path))
    .map_with(|value, e| (value, e.span()))
    .labelled("value");

    let const_item = just(Token::Const)
        .ignore_then(ident)
        .then_ignore(just(Token::Colon))
        .then(ty)
        .then_ignore(just(Token::Equal))
        .then(value)
        .then_ignore(just(Token::Semicolon))
        .map(|((name, ty), value)| Item::Const(Const { name, ty, value }));

    let repr = just(Token::Colon)
        .ignore_then(
            ident.try_map(|(name, span), _: Span| match Prim::from_name(name) {
                Some(prim) => Ok((prim, span)),
                None => Err(Rich::custom(
                    span,
                    format!("`{name}` is not a builtin type"),
                )),
            }),
        )
        .or_not();

    let variant = ident
        .then(
            just(Token::Equal)
                .ignore_then(select! { Token::Int(value) = e => (value, e.span()) })
                .or_not(),
        )
        .map(|(name, value)| Variant { name, value });

    let enum_item = just(Token::Enum)
        .ignore_then(ident)
        .then(repr)
        .then(
            variant
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(just(Token::BraceOpen), just(Token::BraceClose)),
        )
        .map(|((name, repr), variants)| {
            Item::Enum(Enum {
                name,
                repr,
                variants,
            })
        });

    let item = recursive(|item| {
        let member = choice((
            item.map(Member::Item),
            field
                .clone()
                .then_ignore(just(Token::Comma))
                .map(Member::Field),
        ));

        // The last field of a body may leave off its trailing comma.
        let body = member
            .repeated()
            .collect::<Vec<_>>()
            .then(field.or_not())
            .delimited_by(just(Token::BraceOpen), just(Token::BraceClose))
            .map(|(members, last)| collect_body(members, last))
            .boxed();

        let struct_item = just(Token::Struct)
            .ignore_then(ident)
            .then(body.clone())
            .map(|(name, body)| Item::Struct(Struct { name, body }));

        let union_item = just(Token::Union)
            .ignore_then(ident)
            .then(body.clone())
            .map(|(name, body)| Item::Union(Union { name, body }));

        let message_item = just(Token::Message)
            .ignore_then(ident)
            .then(just(Token::Arrow).ignore_then(path.clone()).or_not())
            .then(body)
            .map(|((name, response), body)| {
                Item::Message(Message {
                    name,
                    response,
                    body,
                })
            });

        choice((const_item, enum_item, struct_item, union_item, message_item))
            .labelled("declaration")
            .boxed()
    });

    let namespace = just(Token::Namespace)
        .ignore_then(path.clone())
        .then_ignore(just(Token::Semicolon));

    let use_ = just(Token::Use)
        .ignore_then(path)
        .then_ignore(just(Token::Semicolon));

    namespace
        .then(use_.repeated().collect::<Vec<_>>())
        .then(
            item.recover_with(skip_then_retry_until(any().ignored(), end()))
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then_ignore(end().recover_with(skip_until(any().ignored(), end(), || ())))
        .map(|((namespace, uses), items)| Schema {
            namespace,
            uses,
            items,
        })
}

enum Member<'src> {
    Field(Field<'src>),
    Item(Item<'src>),
}

fn collect_body<'src>(members: Vec<Member<'src>>, last: Option<Field<'src>>) -> Body<'src> {
    let mut body = Body::default();
    for member in members {
        match member {
            Member::Field(field) => body.fields.push(field),
            Member::Item(item) => body.items.push(item),
        }
    }
    body.fields.extend(last);
    body
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn schema_of(src: &str) -> Schema<'_> {
        let (tokens, lex_errors) = lex(src);
        assert!(
            lex_errors.is_empty(),
            "unexpected lex errors: {lex_errors:?}"
        );
        let (schema, errors) = parse(src, &tokens);
        assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
        schema.expect("a schema")
    }

    fn errors_of(src: &str) -> Vec<ParseError> {
        let (tokens, lex_errors) = lex(src);
        assert!(
            lex_errors.is_empty(),
            "unexpected lex errors: {lex_errors:?}"
        );
        let (_, errors) = parse(src, &tokens);
        assert!(!errors.is_empty(), "expected parse errors");
        errors
    }

    fn item<'a, 'src>(schema: &'a Schema<'src>, name: &str) -> &'a Item<'src> {
        schema
            .items
            .iter()
            .find(|item| item.name().0 == name)
            .unwrap_or_else(|| panic!("no declaration named `{name}`"))
    }

    fn fields<'src>(item: &Item<'src>) -> Vec<String> {
        item.body()
            .expect("a body")
            .fields
            .iter()
            .map(|field| {
                let optional = if field.optional { "?" } else { "" };
                format!("{}: {}{}", field.name.0, field.ty, optional)
            })
            .collect()
    }

    const SAMPLE1: &str = include_str!("../tests/data/sample1.uju");
    const SAMPLE2: &str = include_str!("../tests/data/sample2.uju");

    #[test]
    fn declarations_of_every_kind_parse() {
        let schema = schema_of(SAMPLE1);
        assert_eq!(schema.namespace.to_string(), "foo.bar");
        assert!(schema.uses.is_empty());

        let Item::Const(number_one) = item(&schema, "NumberOne") else {
            panic!("expected a const");
        };
        assert_eq!(number_one.ty.to_string(), "i32");
        assert_eq!(number_one.value.0, Value::Int(1));

        let Item::Const(favorite) = item(&schema, "FavoriteColor") else {
            panic!("expected a const");
        };
        assert_eq!(favorite.ty.to_string(), "Color");
        assert_eq!(favorite.value.0.to_string(), "Color.Red");

        let Item::Enum(color) = item(&schema, "Color") else {
            panic!("expected an enum");
        };
        assert_eq!(color.repr.as_ref().map(|(prim, _)| *prim), Some(Prim::U8));
        assert_eq!(
            color
                .variants
                .iter()
                .map(|variant| (variant.name.0, variant.value.as_ref().map(|(v, _)| *v)))
                .collect::<Vec<_>>(),
            [("Red", None), ("Green", None), ("Blue", Some(5))]
        );

        let Item::Enum(numbers) = item(&schema, "Numbers") else {
            panic!("expected an enum");
        };
        assert!(numbers.repr.is_none(), "the repr defaults to u32");

        assert_eq!(item(&schema, "EmptyStruct").body(), Some(&Body::default()));
        assert_eq!(fields(item(&schema, "Velocity")), ["x: f32", "y: f32"]);
        assert_eq!(
            fields(item(&schema, "SomeUnion")),
            ["v: vec<i32>", "s: string", "vel: Velocity"]
        );
    }

    #[test]
    fn builtin_types_are_recognized() {
        let schema = schema_of(SAMPLE1);
        assert_eq!(
            fields(item(&schema, "Scalars")),
            [
                "uint8: u8",
                "uint16: u16",
                "uint32: u32",
                "uint64: u64",
                "int8: i8",
                "int16: i16",
                "int32: i32",
                "int64: i64",
                "boolean: bool",
                "float32: f32",
                "float64: f64",
                "ts: timestamp",
                "it: interval",
                "et: entity",
                "uet: uentity",
                "id: uuid",
            ]
        );
    }

    #[test]
    fn containers_nest_and_only_fields_are_optional() {
        let schema = schema_of(SAMPLE1);
        assert_eq!(
            fields(item(&schema, "NonScalars")),
            [
                "v: vec<i32>",
                "s: set<i32>",
                "m: map<i32, i32>",
                "ar: array<i32, 4>",
                "v1: vec<Position>",
                "v2: vec<Velocity>",
                "v2d: vec<vec<Position>>",
                "vs: vec<set<i32>>",
                "vo: vec<i32>?",
                "s1: set<Position>",
                "m1: map<u32, Position>",
                "m2: map<u8, vec<Position>>",
                "str: string",
                "b: bytes",
            ]
        );

        assert_eq!(
            errors_of("namespace a;\nstruct S { v: vec<i32?> }\n")[0].message,
            "found '?' expected '>'"
        );
    }

    #[test]
    fn messages_carry_a_response_and_nested_declarations() {
        let schema = schema_of(SAMPLE1);

        let Item::Message(request) = item(&schema, "MyRequest") else {
            panic!("expected a message");
        };
        assert_eq!(
            request.response.as_ref().map(Path::to_string),
            Some("MyResponse".to_string())
        );
        assert_eq!(fields(item(&schema, "MyRequest")), ["foo: i32"]);

        let Item::Message(response) = item(&schema, "MyResponse") else {
            panic!("expected a message");
        };
        assert!(response.response.is_none());
        assert_eq!(fields(item(&schema, "MyResponse")), ["result: Result"]);

        let [Item::Enum(result)] = &response.body.items[..] else {
            panic!("expected a nested enum");
        };
        assert_eq!(result.name.0, "Result");
        assert_eq!(result.variants.len(), 2);
    }

    #[test]
    fn imports_and_qualified_types_parse() {
        let schema = schema_of(SAMPLE2);
        assert_eq!(schema.namespace.to_string(), "hello");
        assert_eq!(
            schema.uses.iter().map(Path::to_string).collect::<Vec<_>>(),
            ["foo.bar"]
        );
        assert_eq!(
            fields(item(&schema, "ExternalTypes")),
            ["velocity: Velocity", "position: foo.bar.Position"]
        );
    }

    #[test]
    fn spans_point_back_at_the_source() {
        let src = "namespace a;\nstruct S {\n    m: map<u32, vec<Position>>,\n}\n";
        let schema = schema_of(src);
        let field = &item(&schema, "S").body().expect("a body").fields[0];

        assert_eq!(&src[field.name.1.clone()], "m");
        assert_eq!(&src[field.ty.span.clone()], "map<u32, vec<Position>>");

        let TypeKind::Map(_, value) = &field.ty.kind else {
            panic!("expected a map");
        };
        assert_eq!(&src[value.span.clone()], "vec<Position>");
    }

    #[test]
    fn a_trailing_comma_is_optional() {
        for body in ["{ a: i32, b: i32 }", "{ a: i32, b: i32, }"] {
            let src = format!("namespace a;\nstruct S {body}\n");
            assert_eq!(fields(item(&schema_of(&src), "S")), ["a: i32", "b: i32"]);
        }
        assert_eq!(
            errors_of("namespace a;\nstruct S { a: i32 b: i32 }\n")[0].message,
            "found 'b' expected '?', ',', or '}'"
        );
    }

    #[test]
    fn malformed_types_are_reported() {
        assert_eq!(
            errors_of("namespace a;\nstruct S { v: vec }\n")[0].message,
            "found '}' expected '<'"
        );
        assert_eq!(
            errors_of("namespace a;\nstruct S { v: array<i32, -1> }\n")[0].message,
            "`-1` is not a valid array length"
        );
        assert_eq!(
            errors_of("namespace a;\nenum E: Color { A }\n")[0].message,
            "`Color` is not a builtin type"
        );
    }

    #[test]
    fn parsing_resumes_at_the_next_declaration() {
        let src = "namespace a;\nstruct Bad { x: }\nstruct Good { y: i32 }\n";
        let (tokens, _) = lex(src);
        let (schema, errors) = parse(src, &tokens);

        let schema = schema.expect("a schema despite the error");
        assert_eq!(
            schema
                .items
                .iter()
                .map(|item| item.name().0)
                .collect::<Vec<_>>(),
            ["Good"]
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(&src[errors[0].span.clone()], "}");
    }

    #[test]
    fn a_namespace_is_required() {
        let (tokens, _) = lex("struct S {}\n");
        let (schema, errors) = parse("struct S {}\n", &tokens);
        assert!(schema.is_none());
        assert_eq!(errors.len(), 1);
    }
}
