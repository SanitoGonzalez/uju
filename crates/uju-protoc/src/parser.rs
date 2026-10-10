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

    let number = select! {
        Token::Int(value) => Number::Int(value),
        Token::Float(value) => Number::Float(value),
    }
    .map_with(|value, e| (value, e.span()))
    .labelled("number");

    let constraint = ident
        .then(just(Token::Equal).ignore_then(number).or_not())
        .map_with(|(name, value), e| Constraint {
            name,
            value,
            span: e.span(),
        })
        .labelled("constraint");

    let constraints = constraint
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just(Token::BracketOpen), just(Token::BracketClose))
        .or_not()
        .map(Option::unwrap_or_default);

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
            .ignore_then(arg.clone())
            .map(|item| TypeKind::Set(Box::new(item)));

        let option_ty = just(Token::Ident("option"))
            .ignore_then(arg)
            .map(|item| TypeKind::Option(Box::new(item)));

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
                [(name, _)] if matches!(name, "vec" | "set" | "map" | "array" | "option") => Err(
                    Rich::custom(span, format!("`{name}` requires type arguments")),
                ),
                _ => Ok(TypeKind::Named(path)),
            });

        choice((
            vec_ty, set_ty, map_ty, array_ty, option_ty, prim_ty, named_ty,
        ))
        .then(constraints)
        .map_with(|(kind, constraints), e| Type {
            kind,
            constraints,
            span: e.span(),
        })
        .labelled("type")
        .boxed()
    });

    let field = ident
        .then_ignore(just(Token::Colon))
        .then(ty)
        .map(|(name, ty)| Field { name, ty })
        .boxed();

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

        let direction = ident
            .try_map(|(name, span), _: Span| match Direction::from_name(name) {
                Some(direction) => Ok((direction, span)),
                None => Err(Rich::custom(
                    span,
                    format!("`{name}` is not a direction; expected `in`, `out`, or `bi`"),
                )),
            })
            .delimited_by(just(Token::BracketOpen), just(Token::BracketClose));

        let message_item = just(Token::Message)
            .ignore_then(direction)
            .then(ident)
            .then(body)
            .map(|((direction, name), body)| {
                Item::Message(Message {
                    direction,
                    name,
                    body,
                })
            });

        choice((enum_item, struct_item, message_item))
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
            .map(|field| format!("{}: {}", field.name.0, field.ty))
            .collect()
    }

    const SAMPLE1: &str = include_str!("../tests/data/sample1.uju");
    const SAMPLE2: &str = include_str!("../tests/data/sample2.uju");

    #[test]
    fn declarations_of_every_kind_parse() {
        let schema = schema_of(SAMPLE1);
        assert_eq!(schema.namespace.to_string(), "foo.bar");
        assert!(schema.uses.is_empty());

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
        assert_eq!(fields(item(&schema, "Position")), ["x: f32", "y: f32"]);
    }

    #[test]
    fn builtin_types_are_recognized() {
        let names = [
            "u8",
            "u16",
            "u32",
            "u64",
            "i8",
            "i16",
            "i32",
            "i64",
            "bool",
            "f32",
            "f64",
            "timestamp",
            "interval",
            "entity",
            "uentity",
            "uuid",
            "string",
            "bytes",
        ];
        let body: String = names
            .iter()
            .map(|name| format!("f_{name}: {name}, "))
            .collect();
        let src = format!("namespace a;\nmessage[in] M {{ {body} }}\n");
        let schema = schema_of(&src);
        assert_eq!(
            fields(item(&schema, "M")),
            names.map(|name| format!("f_{name}: {name}"))
        );
        let Some(Field { ty, .. }) = item(&schema, "M").body().unwrap().fields.first() else {
            panic!("expected a field");
        };
        assert_eq!(ty.kind, TypeKind::Prim(Prim::U8));
    }

    #[test]
    fn containers_and_options_nest() {
        let src = concat!(
            "namespace a;\n",
            "message[bi] M {\n",
            "    v: vec<i32>,\n",
            "    s: set<i32>,\n",
            "    m: map<i32, i32>,\n",
            "    ar: array<i32, 4>,\n",
            "    v2d: vec<vec<Position>>,\n",
            "    m2: map<u8, vec<Position>>,\n",
            "    o: option<i32>,\n",
            "    op: option<foo.Position>,\n",
            "}\n",
        );
        assert_eq!(
            fields(item(&schema_of(src), "M")),
            [
                "v: vec<i32>",
                "s: set<i32>",
                "m: map<i32, i32>",
                "ar: array<i32, 4>",
                "v2d: vec<vec<Position>>",
                "m2: map<u8, vec<Position>>",
                "o: option<i32>",
                "op: option<foo.Position>",
            ]
        );
    }

    #[test]
    fn messages_carry_a_direction_and_nested_declarations() {
        let schema = schema_of(SAMPLE1);

        let direction = |name: &str| {
            let Item::Message(message) = item(&schema, name) else {
                panic!("expected a message");
            };
            message.direction.0
        };
        assert_eq!(direction("Ping"), Direction::Bi);
        assert_eq!(direction("MyRequest"), Direction::In);
        assert_eq!(direction("MyResponse"), Direction::Out);
        assert_eq!(
            fields(item(&schema, "MyRequest")),
            [
                "foo: i32[min=-100, max=100]",
                "name: string[min_length=2, max_length=12]",
                "addresses: vec<string[max_length=16]>[max_length=14]",
            ]
        );
        assert_eq!(fields(item(&schema, "MyResponse")), ["result: Result"]);

        let Item::Message(response) = item(&schema, "MyResponse") else {
            panic!("expected a message");
        };
        let [Item::Enum(result)] = &response.body.items[..] else {
            panic!("expected a nested enum");
        };
        assert_eq!(result.name.0, "Result");
        assert_eq!(result.variants.len(), 2);
    }

    #[test]
    fn constraints_bind_to_the_type_they_follow() {
        let src = concat!(
            "namespace a;\n",
            "message[in] M {\n",
            "    foo: i32[min=-100, max=100],\n",
            "    f: f32[infinite, min=0.5,],\n",
            "    v: vec<string[max_length=14]>[min_length=1],\n",
            "    o: option<u8[max=3]>,\n",
            "}\n",
        );
        let schema = schema_of(src);
        assert_eq!(
            fields(item(&schema, "M")),
            [
                "foo: i32[min=-100, max=100]",
                "f: f32[infinite, min=0.5]",
                "v: vec<string[max_length=14]>[min_length=1]",
                "o: option<u8[max=3]>",
            ]
        );

        let field = &item(&schema, "M").body().unwrap().fields[2];
        assert_eq!(
            &src[field.ty.span.clone()],
            "vec<string[max_length=14]>[min_length=1]"
        );
        assert_eq!(field.ty.constraints[0].name.0, "min_length");
        let TypeKind::Vec(element) = &field.ty.kind else {
            panic!("expected a vec");
        };
        assert_eq!(element.constraints[0].name.0, "max_length");
        assert_eq!(
            element.constraints[0].value.as_ref().map(|(v, _)| *v),
            Some(Number::Int(14))
        );
    }

    #[test]
    fn malformed_constraints_are_reported() {
        assert_eq!(
            errors_of("namespace a;\nstruct S { x: i32[] }\n")[0].message,
            "found ']' expected constraint"
        );
        assert_eq!(
            errors_of("namespace a;\nstruct S { x: i32[min=] }\n")[0].message,
            "found ']' expected number"
        );
    }

    #[test]
    fn a_direction_is_required() {
        assert_eq!(
            errors_of("namespace a;\nmessage M {}\n")[0].message,
            "found 'M' expected '['"
        );
        assert_eq!(
            errors_of("namespace a;\nmessage[up] M {}\n")[0].message,
            "`up` is not a direction; expected `in`, `out`, or `bi`"
        );
    }

    #[test]
    fn constants_are_not_declarations() {
        assert_eq!(
            errors_of("namespace a;\nconst A: i32 = 1;\n")[0].message,
            "found 'const' expected 'use', declaration, or end of input"
        );
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
            ["color: Color", "position: foo.bar.Position"]
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
            "found 'b' expected '[', ',', or '}'"
        );
    }

    #[test]
    fn malformed_types_are_reported() {
        assert_eq!(
            errors_of("namespace a;\nstruct S { v: vec }\n")[0].message,
            "found '}' expected '<'"
        );
        assert_eq!(
            errors_of("namespace a;\nstruct S { o: option }\n")[0].message,
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
