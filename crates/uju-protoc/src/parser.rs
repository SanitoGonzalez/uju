pub mod ast;

use std::fmt;

use chumsky::error::Rich;
use chumsky::input::{Emitter, Input, ValueInput};
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

    let literal = select! {
        Token::Int(value) => Literal::Int(value),
        Token::Float(value) => Literal::Float(value),
        Token::Str(value) => Literal::String(value),
        Token::Duration((count, unit)) => Literal::Duration(count, unit),
        Token::Timestamp(micros) => Literal::Timestamp(micros),
        Token::Ident("true") => Literal::Bool(true),
        Token::Ident("false") => Literal::Bool(false),
    }
    .map_with(|value, e| (value, e.span()))
    .labelled("literal");

    // An attribute's `(a, b)`, or a nested call's. Once the `(` is read the
    // arguments must follow, rather than leaving a bare name before it.
    let args = recursive(|args| {
        let call = path
            .clone()
            .then(maybe_args(args))
            .validate(|(path, args), e, emitter| match args {
                None => AttributeArg::Path(path),
                Some(args) => AttributeArg::Call {
                    args: reject_empty(&path.to_string(), args, emitter),
                    path,
                    span: e.span(),
                },
            });
        choice((literal.map(AttributeArg::Literal), call))
            .labelled("argument")
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::ParenOpen), just(Token::ParenClose))
            .map_with(|args, e| (args, e.span()))
            .boxed()
    });

    let attribute = just(Token::At)
        .ignore_then(ident.labelled("attribute name"))
        .then(maybe_args(args))
        .validate(|(name, args), e, emitter| Attribute {
            args: args.map_or_else(Vec::new, |args| {
                reject_empty(&format!("@{}", name.0), args, emitter)
            }),
            name,
            span: e.span(),
        })
        .labelled("attribute")
        .boxed();

    let const_item = just(Token::Const)
        .ignore_then(ident)
        .then_ignore(just(Token::Colon))
        .then(ty.clone())
        .then_ignore(just(Token::Equal))
        .then(literal)
        .map(|((name, ty), value)| {
            Item::Const(Const {
                attrs: Vec::new(),
                name,
                ty,
                value,
            })
        });

    let field = without_attrs(
        attribute.clone(),
        "fields",
        ident
            .then_ignore(just(Token::Colon))
            .then(ty.clone())
            .map(|(name, ty)| Field { name, ty }),
    )
    .boxed();

    // `type`, `union`, and `hub` are keywords only where a declaration
    // starts, so they stay usable as names, as in a field `type: u8`.
    let type_item = just(Token::Ident("type"))
        .ignore_then(ident)
        .then_ignore(just(Token::Equal))
        .then(ty.clone())
        .map(|(name, target)| {
            Item::Type(TypeDecl {
                attrs: Vec::new(),
                name,
                target,
            })
        });

    let variant_fields = field
        .clone()
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(just(Token::BraceOpen), just(Token::BraceClose))
        .map_with(|fields, e| VariantData::Fields {
            fields,
            span: e.span(),
        });
    let variant_type = ty
        .clone()
        .delimited_by(just(Token::ParenOpen), just(Token::ParenClose))
        .map(VariantData::Type);
    let union_variant = without_attrs(
        attribute.clone(),
        "union variants",
        ident
            .then(choice((variant_fields, variant_type)).or_not())
            .map(|(name, data)| UnionVariant {
                name,
                data: data.unwrap_or(VariantData::Unit),
            }),
    );

    let union_item = just(Token::Ident("union"))
        .ignore_then(ident)
        .then(
            union_variant
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(just(Token::BraceOpen), just(Token::BraceClose)),
        )
        .map(|(name, variants)| {
            Item::Union(Union {
                attrs: Vec::new(),
                name,
                variants,
            })
        });

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

    let variant = without_attrs(
        attribute.clone(),
        "enum variants",
        ident
            .then(
                just(Token::Equal)
                    .ignore_then(select! { Token::Int(value) = e => (value, e.span()) })
                    .or_not(),
            )
            .map(|(name, value)| Variant { name, value }),
    );

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
                attrs: Vec::new(),
                name,
                repr,
                variants,
            })
        });

    let item = recursive(|item| {
        // A field takes a comma, except the last one of a body. A field
        // takes the attributes before it, rejecting them, so it is tried
        // before them.
        let member = choice((
            item.map(Member::Item),
            field
                .clone()
                .then_ignore(
                    just(Token::Comma)
                        .ignored()
                        .or(just(Token::BraceClose).rewind().ignored()),
                )
                .map(Member::Field),
            attribute.clone().map(Member::Attr),
        ));

        let body = member
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just(Token::BraceOpen), just(Token::BraceClose))
            .validate(|members, _, emitter| {
                let body = collect_body(members, emitter);
                for item in &body.items {
                    if let Item::Hub(hub) = item {
                        emitter.emit(Rich::custom(
                            hub.name.1.clone(),
                            "a hub kind must be declared at the top level of a namespace",
                        ));
                    }
                }
                body
            })
            .boxed();

        let struct_item = just(Token::Struct)
            .ignore_then(ident)
            .then(body.clone())
            .map(|(name, body)| {
                Item::Struct(Struct {
                    attrs: Vec::new(),
                    name,
                    body,
                })
            });

        // `sim` or `hub<Kind>`. Role names are not keywords.
        let endpoint = ident
            .labelled("role")
            .then(
                path.clone()
                    .delimited_by(just(Token::Lt), just(Token::Gt))
                    .or_not(),
            )
            .try_map(|((name, name_span), kind), span: Span| {
                let role = match Role::from_name(name) {
                    Some(role) => role,
                    None if name == "coordinator" => {
                        return Err(Rich::custom(
                            name_span,
                            "`coordinator` cannot be an endpoint; coordinator speaks only uju's own messages",
                        ));
                    }
                    None => {
                        return Err(Rich::custom(
                            name_span,
                            format!(
                                "`{name}` is not a role; expected `client`, `entry`, `edge`, `sim`, or `hub`"
                            ),
                        ));
                    }
                };
                if role != Role::Hub && kind.is_some() {
                    return Err(Rich::custom(
                        span,
                        format!("only `hub` takes a kind, as `hub<Party>`; write `{name}`"),
                    ));
                }
                Ok(Endpoint { role, kind, span })
            });

        let side = endpoint
            .separated_by(just(Token::Pipe))
            .at_least(1)
            .collect::<Vec<_>>();

        let endpoints = side
            .clone()
            .then(choice((
                just(Token::Arrow).to(false),
                just(Token::BiArrow).to(true),
            )))
            .then(side)
            .delimited_by(just(Token::ParenOpen), just(Token::ParenClose))
            .map_with(|((from, both), to), e| Endpoints {
                from,
                to,
                both,
                span: e.span(),
            });

        let message_item = just(Token::Message)
            .ignore_then(endpoints)
            .then(ident)
            .then(body.clone())
            .map(|((endpoints, name), body)| {
                Item::Message(Message {
                    attrs: Vec::new(),
                    endpoints,
                    name,
                    body,
                })
            });

        // `hub(Key) Name { ... }`. Parsed in any body, so that `body` can
        // report one nested there.
        let hub_item = just(Token::Ident("hub"))
            .ignore_then(
                ty.clone()
                    .delimited_by(just(Token::ParenOpen), just(Token::ParenClose)),
            )
            .then(ident)
            .then(body)
            .map(|((key, name), body)| {
                Item::Hub(Hub {
                    attrs: Vec::new(),
                    key,
                    name,
                    body,
                })
            });

        choice((
            enum_item,
            struct_item,
            message_item,
            hub_item,
            const_item,
            union_item.clone(),
            type_item.clone(),
        ))
        .labelled("declaration")
        .boxed()
    });

    let namespace = without_attrs(
        attribute.clone(),
        "`namespace`",
        just(Token::Namespace).ignore_then(path.clone()),
    );

    let use_ = without_attrs(
        attribute.clone(),
        "`use`",
        just(Token::Use).ignore_then(path),
    );

    let items = choice((item.map(Member::Item), attribute.map(Member::Attr)))
        .labelled("declaration")
        .recover_with(skip_then_retry_until(any().ignored(), end()))
        .repeated()
        .collect::<Vec<_>>()
        .validate(|members, _, emitter| collect_body(members, emitter).items);

    namespace
        .then(use_.repeated().collect::<Vec<_>>())
        .then(items)
        .then_ignore(end().recover_with(skip_until(any().ignored(), end(), || ())))
        .map(|((namespace, uses), items)| Schema {
            namespace,
            uses,
            items,
        })
}

/// `inner`, rejecting any attributes written before it, which are not
/// allowed on `what`.
fn without_attrs<'tok, 'src: 'tok, I, O>(
    attribute: impl Parser<'tok, I, Attribute<'src>, Extra<'tok, 'src>> + Clone,
    what: &'static str,
    inner: impl Parser<'tok, I, O, Extra<'tok, 'src>> + Clone,
) -> impl Parser<'tok, I, O, Extra<'tok, 'src>> + Clone
where
    I: ValueInput<'tok, Token = Token<'src>, Span = Span>,
{
    attribute
        .repeated()
        .at_least(1)
        .ignored()
        .map_with(|(), e| e.span())
        .or_not()
        .then(inner)
        .validate(move |(attrs, inner), _, emitter| {
            if let Some(span) = attrs {
                emitter.emit(Rich::custom(
                    span,
                    format!("attributes are not allowed on {what}"),
                ));
            }
            inner
        })
}

/// `args`, if a `(` comes next.
fn maybe_args<'tok, 'src: 'tok, I, O>(
    args: impl Parser<'tok, I, O, Extra<'tok, 'src>> + Clone,
) -> impl Parser<'tok, I, Option<O>, Extra<'tok, 'src>> + Clone
where
    I: ValueInput<'tok, Token = Token<'src>, Span = Span>,
{
    choice((args.map(Some), just(Token::ParenOpen).not().map(|()| None)))
}

/// The arguments in `args`, reporting `()` after `name`.
fn reject_empty<'tok, 'src, T>(
    name: &str,
    (args, span): Spanned<Vec<T>>,
    emitter: &mut Emitter<Rich<'tok, Token<'src>, Span>>,
) -> Vec<T> {
    if args.is_empty() {
        emitter.emit(Rich::custom(
            span,
            format!("empty parentheses; write `{name}` without them"),
        ));
    }
    args
}

/// What a body, or the top level, is made of: attributes are parsed on
/// their own and attached to the declaration after them.
enum Member<'src> {
    Attr(Attribute<'src>),
    Field(Field<'src>),
    Item(Item<'src>),
}

fn collect_body<'tok, 'src>(
    members: Vec<Member<'src>>,
    emitter: &mut Emitter<Rich<'tok, Token<'src>, Span>>,
) -> Body<'src> {
    let mut body = Body::default();
    let mut attrs = Vec::new();
    for member in members {
        match member {
            Member::Attr(attr) => attrs.push(attr),
            // A field already rejected the attributes before it.
            Member::Field(field) => body.fields.push(field),
            Member::Item(mut item) => {
                *item.attrs_mut() = std::mem::take(&mut attrs);
                body.items.push(item);
            }
        }
    }
    if let (Some(first), Some(last)) = (attrs.first(), attrs.last()) {
        emitter.emit(Rich::custom(
            first.span.start..last.span.end,
            "an attribute must go above a declaration",
        ));
    }
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
    use crate::lexer::{LexError, lex};

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

    const EXAMPLE1: &str = include_str!("../tests/data/example1.uju");
    const EXAMPLE2: &str = include_str!("../tests/data/example2.uju");

    #[test]
    fn declarations_of_every_kind_parse() {
        let schema = schema_of(EXAMPLE1);
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

        let Item::Type(level) = item(&schema, "Level") else {
            panic!("expected a type");
        };
        assert_eq!(level.target.to_string(), "u8[max=100]");
        let Item::Union(op) = item(&schema, "PartyOp") else {
            panic!("expected a union");
        };
        assert!(matches!(
            op.variants
                .iter()
                .map(|variant| &variant.data)
                .collect::<Vec<_>>()[..],
            [
                VariantData::Fields { .. },
                VariantData::Type(_),
                VariantData::Type(_),
                VariantData::Unit
            ]
        ));
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
            "duration",
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
        let src = format!("namespace a\nmessage(client -> sim) M {{ {body} }}\n");
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
            "namespace a\n",
            "message(client <-> sim) M {\n",
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
    fn messages_carry_endpoints_and_nested_declarations() {
        let schema = schema_of(EXAMPLE1);

        let endpoints = |name: &str| {
            let Item::Message(message) = item(&schema, name) else {
                panic!("expected a message");
            };
            let side = |side: &[Endpoint<'_>]| -> Vec<String> {
                side.iter().map(|endpoint| endpoint.to_string()).collect()
            };
            (
                side(&message.endpoints.from),
                message.endpoints.both,
                side(&message.endpoints.to),
            )
        };
        assert_eq!(
            endpoints("Ping"),
            (
                vec!["client".into()],
                true,
                vec!["entry".into(), "edge".into()]
            )
        );
        assert_eq!(
            endpoints("MyRequest"),
            (vec!["client".into()], false, vec!["sim".into()])
        );
        assert_eq!(
            endpoints("Deposit"),
            (
                vec!["client".into(), "sim".into()],
                false,
                vec!["hub<GuildBank>".into()]
            )
        );
        assert_eq!(
            endpoints("GuildNotice"),
            (
                vec!["hub<Guild>".into(), "hub<GuildBank>".into()],
                false,
                vec!["client".into()]
            )
        );
        assert_eq!(
            fields(item(&schema, "MyRequest")),
            [
                "foo: i32[min=-100, max=100]",
                "name: string[min_length=2, max_length=12]",
                "addresses: vec<string[max_length=16]>[max_length=14]",
            ]
        );
        assert_eq!(fields(item(&schema, "MyResponse")), ["result: Result"]);

        let Item::Message(my_response) = item(&schema, "MyResponse") else {
            panic!("expected a message");
        };
        let [Item::Enum(result)] = &my_response.body.items[..] else {
            panic!("expected a nested enum");
        };
        assert_eq!(result.name.0, "Result");
        assert_eq!(result.variants.len(), 2);
    }

    #[test]
    fn constraints_bind_to_the_type_they_follow() {
        let src = concat!(
            "namespace a\n",
            "message(client -> sim) M {\n",
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
            errors_of("namespace a\nstruct S { x: i32[] }\n")[0].message,
            "found ']' expected constraint"
        );
        assert_eq!(
            errors_of("namespace a\nstruct S { x: i32[min=] }\n")[0].message,
            "found ']' expected number"
        );
    }

    #[test]
    fn endpoints_are_required() {
        assert_eq!(
            errors_of("namespace a\nmessage M {}\n")[0].message,
            "found 'M' expected '('"
        );
        assert_eq!(
            errors_of("namespace a\nmessage() M {}\n")[0].message,
            "found ')' expected role"
        );
        assert_eq!(
            errors_of("namespace a\nmessage(client) M {}\n")[0].message,
            "found ')' expected '<', '|', '->', or '<->'"
        );
        assert_eq!(
            errors_of("namespace a\nmessage(client ->) M {}\n")[0].message,
            "found ')' expected role"
        );
        assert_eq!(
            errors_of("namespace a\nmessage(client | -> sim) M {}\n")[0].message,
            "found '->' expected role"
        );
    }

    #[test]
    fn endpoints_are_roles() {
        assert_eq!(
            errors_of("namespace a\nmessage(client -> server) M {}\n")[0].message,
            "`server` is not a role; expected `client`, `entry`, `edge`, `sim`, or `hub`"
        );
        assert_eq!(
            errors_of("namespace a\nmessage(coordinator -> sim) M {}\n")[0].message,
            "`coordinator` cannot be an endpoint; coordinator speaks only uju's own messages"
        );
        let src = "namespace a\nmessage(client -> sim<P>) M {}\n";
        let error = &errors_of(src)[0];
        assert_eq!(
            error.message,
            "only `hub` takes a kind, as `hub<Party>`; write `sim`"
        );
        assert_eq!(&src[error.span.clone()], "sim<P>");
    }

    #[test]
    fn endpoints_list_roles_and_hub_kinds() {
        let src = "namespace a\nmessage(client|hub<x.P> <-> hub | sim) M {}\n";
        let schema = schema_of(src);
        let Item::Message(message) = item(&schema, "M") else {
            panic!("expected a message");
        };
        let endpoints = &message.endpoints;
        assert_eq!(
            &src[endpoints.span.clone()],
            "(client|hub<x.P> <-> hub | sim)"
        );
        assert!(endpoints.both);
        let roles = |side: &[Endpoint<'_>]| -> Vec<Role> {
            side.iter().map(|endpoint| endpoint.role).collect()
        };
        assert_eq!(roles(&endpoints.from), [Role::Client, Role::Hub]);
        assert_eq!(roles(&endpoints.to), [Role::Hub, Role::Sim]);
        assert_eq!(&src[endpoints.from[1].span.clone()], "hub<x.P>");
        assert_eq!(
            endpoints.from[1].kind.as_ref().map(|kind| kind.to_string()),
            Some("x.P".into())
        );
        assert_eq!(endpoints.to[0].kind, None);
        // Role names are not keywords.
        let schema = schema_of("namespace a\nstruct client { sim: u8, hub: u8, edge: u8 }\n");
        assert_eq!(
            fields(item(&schema, "client")),
            ["sim: u8", "hub: u8", "edge: u8"]
        );
    }

    fn const_of<'a, 'src>(items: &'a [Item<'src>], name: &str) -> &'a Const<'src> {
        items
            .iter()
            .find_map(|item| match item {
                Item::Const(item) if item.name.0 == name => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no const named `{name}`"))
    }

    #[test]
    fn consts_parse_at_the_top_and_in_bodies() {
        let src = concat!(
            "namespace a\n",
            "const Count: u8 = 3\n",
            "const Rate: f32 = -0.5\n",
            "const On: bool = true\n",
            "const Off: bool = false\n",
            "const Greeting: string = \"hi\\n\"\n",
            "message(client -> sim) M {\n",
            "    const Inner: u16 = 7\n",
            "    x: u8,\n",
            "    const Last: i8 = -1\n",
            "}\n",
        );
        let schema = schema_of(src);
        let value = |item: &Const<'_>| item.value.0.clone();
        assert_eq!(value(const_of(&schema.items, "Count")), Literal::Int(3));
        assert_eq!(value(const_of(&schema.items, "Rate")), Literal::Float(-0.5));
        assert_eq!(value(const_of(&schema.items, "On")), Literal::Bool(true));
        assert_eq!(value(const_of(&schema.items, "Off")), Literal::Bool(false));
        let greeting = const_of(&schema.items, "Greeting");
        assert_eq!(value(greeting), Literal::String("hi\n".into()));
        assert_eq!(greeting.ty.to_string(), "string");
        assert_eq!(&src[greeting.value.1.clone()], "\"hi\\n\"");

        let message = item(&schema, "M");
        assert_eq!(fields(message), ["x: u8"]);
        let body = &message.body().unwrap().items;
        assert_eq!(value(const_of(body, "Inner")), Literal::Int(7));
        assert_eq!(value(const_of(body, "Last")), Literal::Int(-1));
        assert_eq!(message.body().unwrap().items[0].keyword(), "const");
    }

    #[test]
    fn consts_take_duration_and_timestamp_literals() {
        let src = concat!(
            "namespace a\n",
            "const Micro: duration = 250us\n",
            "const Milli: duration = 1000ms\n",
            "const Second: duration = 30s\n",
            "const Minute: duration = 5m\n",
            "const Hour: duration = 2h\n",
            "const Start: timestamp = 2026-11-07T12:00:00Z\n",
            "const Precise: timestamp = 2026-11-07T12:00:00.250125Z\n",
        );
        let schema = schema_of(src);
        let value = |name| const_of(&schema.items, name).value.0.clone();
        assert_eq!(value("Micro"), Literal::Duration(250, DurationUnit::Us));
        assert_eq!(value("Milli"), Literal::Duration(1000, DurationUnit::Ms));
        assert_eq!(value("Second"), Literal::Duration(30, DurationUnit::S));
        assert_eq!(value("Minute"), Literal::Duration(5, DurationUnit::M));
        assert_eq!(value("Hour"), Literal::Duration(2, DurationUnit::H));
        assert_eq!(value("Start"), Literal::Timestamp(1_794_052_800_000_000));
        assert_eq!(value("Precise"), Literal::Timestamp(1_794_052_800_250_125));
        let precise = const_of(&schema.items, "Precise");
        assert_eq!(precise.ty.to_string(), "timestamp");
        assert_eq!(&src[precise.value.1.clone()], "2026-11-07T12:00:00.250125Z");
        assert_eq!(precise.value.0.to_string(), "2026-11-07T12:00:00.250125Z");
    }

    #[test]
    fn malformed_consts_are_reported() {
        assert_eq!(
            errors_of("namespace a\nconst A: i32 =\nstruct S {}\n")[0].message,
            "found 'struct' expected literal"
        );
        assert_eq!(
            errors_of("namespace a\nconst A: i32\n")[0].message,
            "found end of input expected '[', or '='"
        );
        assert_eq!(
            errors_of("namespace a\nconst A: bool = yes\n")[0].message,
            "found 'yes' expected literal"
        );
        assert_eq!(
            errors_of("namespace a\nconst A = 1\n")[0].message,
            "found '=' expected ':'"
        );
    }

    #[test]
    fn types_and_unions_parse() {
        let src = concat!(
            "namespace a\n",
            "type PartyId = u64\n",
            "type Level = u8[max=100]\n",
            "union Op {\n",
            "    Add { item: u32, slot: u8 },\n",
            "    Remove(u8[max=9]),\n",
            "    Clear,\n",
            "}\n",
            "union One { Only }\n",
            "message(client -> sim) M {\n",
            "    type Inner = PartyId\n",
            "    union Nested { A(Inner), }\n",
            "    x: Inner,\n",
            "}\n",
        );
        let schema = schema_of(src);

        let Item::Type(party) = item(&schema, "PartyId") else {
            panic!("expected a type");
        };
        assert_eq!(party.target.to_string(), "u64");
        let Item::Type(level) = item(&schema, "Level") else {
            panic!("expected a type");
        };
        assert_eq!(level.target.to_string(), "u8[max=100]");
        assert_eq!(item(&schema, "Level").keyword(), "type");

        let Item::Union(op) = item(&schema, "Op") else {
            panic!("expected a union");
        };
        assert_eq!(item(&schema, "Op").keyword(), "union");
        assert_eq!(
            op.variants
                .iter()
                .map(|variant| variant.name.0)
                .collect::<Vec<_>>(),
            ["Add", "Remove", "Clear"]
        );
        let VariantData::Fields { fields: add, span } = &op.variants[0].data else {
            panic!("expected fields");
        };
        assert_eq!(
            add.iter()
                .map(|field| format!("{}: {}", field.name.0, field.ty))
                .collect::<Vec<_>>(),
            ["item: u32", "slot: u8"]
        );
        assert_eq!(&src[span.clone()], "{ item: u32, slot: u8 }");
        let VariantData::Type(ty) = &op.variants[1].data else {
            panic!("expected a type");
        };
        assert_eq!(ty.to_string(), "u8[max=9]");
        assert_eq!(op.variants[2].data, VariantData::Unit);

        let Item::Union(one) = item(&schema, "One") else {
            panic!("expected a union");
        };
        assert_eq!(one.variants.len(), 1);
        assert_eq!(one.variants[0].data, VariantData::Unit);

        let message = item(&schema, "M");
        assert_eq!(fields(message), ["x: Inner"]);
        let body = &message.body().unwrap().items;
        assert_eq!(
            body.iter().map(Item::keyword).collect::<Vec<_>>(),
            ["type", "union"]
        );
    }

    #[test]
    fn type_and_union_are_keywords_only_where_a_declaration_starts() {
        let src = concat!(
            "namespace a\n",
            "struct S { type: u8, union: u8 }\n",
            "message(client -> sim) M { type: u8, union: u8, }\n",
            "union U { A { type: u8, union: u8 } }\n",
            "type type = u8\n",
            "union union { union }\n",
        );
        let schema = schema_of(src);
        assert_eq!(fields(item(&schema, "S")), ["type: u8", "union: u8"]);
        assert_eq!(fields(item(&schema, "M")), ["type: u8", "union: u8"]);
        assert!(item(&schema, "M").body().unwrap().items.is_empty());
        assert_eq!(item(&schema, "type").keyword(), "type");
        let Item::Union(union) = item(&schema, "union") else {
            panic!("expected a union");
        };
        assert_eq!(union.variants[0].name.0, "union");
    }

    #[test]
    fn hub_kinds_parse() {
        let src = concat!(
            "namespace a\n",
            "type GuildId = u64\n",
            "@cached\n",
            "hub(GuildId) Guild {\n",
            "    const MaxMembers: u8 = 50\n",
            "    enum Rank { Member, Officer }\n",
            "    name: string[max_length=24],\n",
            "    rank: Rank,\n",
            "}\n",
            "hub(array<u8, 4>) Plot {}\n",
            "hub(b.Id) Party { leader: u64 }\n",
        );
        let schema = schema_of(src);

        let Item::Hub(guild) = item(&schema, "Guild") else {
            panic!("expected a hub kind");
        };
        assert_eq!(item(&schema, "Guild").keyword(), "hub");
        assert_eq!(attrs(item(&schema, "Guild")), ["@cached"]);
        assert_eq!(guild.key.to_string(), "GuildId");
        assert_eq!(&src[guild.key.span.clone()], "GuildId");
        assert_eq!(
            fields(item(&schema, "Guild")),
            ["name: string[max_length=24]", "rank: Rank"]
        );
        assert_eq!(
            guild
                .body
                .items
                .iter()
                .map(Item::keyword)
                .collect::<Vec<_>>(),
            ["const", "enum"]
        );

        let Item::Hub(plot) = item(&schema, "Plot") else {
            panic!("expected a hub kind");
        };
        assert_eq!(plot.key.to_string(), "array<u8, 4>");
        assert_eq!(plot.body, Body::default());
        let Item::Hub(party) = item(&schema, "Party") else {
            panic!("expected a hub kind");
        };
        assert_eq!(party.key.to_string(), "b.Id");
        assert_eq!(fields(item(&schema, "Party")), ["leader: u64"]);
    }

    #[test]
    fn hub_is_a_keyword_only_where_a_declaration_starts() {
        let src = concat!(
            "namespace a\n",
            "struct S { hub: u8 }\n",
            "message(client -> sim) M { hub: u8, }\n",
            "union U { hub { hub: u8 } }\n",
            "struct hub {}\n",
            "hub(hub) H { hub: hub }\n",
        );
        let schema = schema_of(src);
        assert_eq!(fields(item(&schema, "S")), ["hub: u8"]);
        assert_eq!(fields(item(&schema, "M")), ["hub: u8"]);
        assert_eq!(item(&schema, "hub").keyword(), "struct");
        let Item::Hub(hub) = item(&schema, "H") else {
            panic!("expected a hub kind");
        };
        assert_eq!(hub.key.to_string(), "hub");
        assert_eq!(fields(item(&schema, "H")), ["hub: hub"]);
    }

    #[test]
    fn a_hub_kind_is_declared_only_at_the_top_level() {
        let nested = "a hub kind must be declared at the top level of a namespace";
        for src in [
            "namespace a\nstruct S { hub(u64) Party {} }\n",
            "namespace a\nmessage(client -> sim) M { x: u8, hub(u64) Party { y: u8 } }\n",
            "namespace a\nhub(u64) Guild { hub(u64) Party {} }\n",
        ] {
            let (tokens, _) = lex(src);
            let (schema, errors) = parse(src, &tokens);
            assert!(schema.is_some(), "parsing goes on");
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert_eq!(&src[errors[0].span.clone()], "Party");
            assert_eq!(errors[0].message, nested);
        }
    }

    #[test]
    fn malformed_hub_kinds_are_reported() {
        let error = |src: &str| {
            let errors = errors_of(src);
            (
                src[errors[0].span.clone()].to_string(),
                errors[0].message.clone(),
            )
        };
        assert_eq!(
            error("namespace a\nhub Guild {}\n"),
            ("Guild".into(), "found 'Guild' expected '('".into())
        );
        assert_eq!(
            error("namespace a\nhub() Guild {}\n"),
            (")".into(), "found ')' expected type".into())
        );
        assert_eq!(
            error("namespace a\nhub(u64, u32) Guild {}\n"),
            (",".into(), "found ',' expected '[', or ')'".into())
        );
        assert_eq!(
            error("namespace a\nhub(u64) {}\n"),
            ("{".into(), "found '{' expected identifier".into())
        );
        assert_eq!(
            error("namespace a\nhub(u64) Guild\n"),
            ("".into(), "found end of input expected '{'".into())
        );
        assert_eq!(
            error("namespace a\nhub(u64) Guild { x: u8 y: u8 }\n"),
            ("y".into(), "found 'y' expected '[', ',', or '}'".into())
        );
    }

    #[test]
    fn empty_braces_parse_as_a_variant_without_fields() {
        // Lowering rejects it, suggesting the bare name.
        let src = "namespace a\nunion U { Add {} }\n";
        let schema = schema_of(src);
        let Item::Union(union) = item(&schema, "U") else {
            panic!("expected a union");
        };
        let VariantData::Fields { fields, span } = &union.variants[0].data else {
            panic!("expected fields");
        };
        assert!(fields.is_empty());
        assert_eq!(&src[span.clone()], "{}");
    }

    #[test]
    fn malformed_types_and_unions_are_reported() {
        assert_eq!(
            errors_of("namespace a\ntype T u8\n")[0].message,
            "found 'u8' expected '='"
        );
        assert_eq!(
            errors_of("namespace a\ntype T =\nstruct S {}\n")[0].message,
            "found 'struct' expected type"
        );
        assert_eq!(
            errors_of("namespace a\nunion U { A(u8, u8) }\n")[0].message,
            "found ',' expected '[', or ')'"
        );
        assert_eq!(
            errors_of("namespace a\nunion U { A() }\n")[0].message,
            "found ')' expected type"
        );
        assert_eq!(
            errors_of("namespace a\nunion U { A B }\n")[0].message,
            "found 'B' expected '{', '(', ',', or '}'"
        );
    }

    #[test]
    fn declarations_take_no_terminator() {
        let src = "namespace a use b type A = u8 type B = u16[max=9] const C: u8 = 1 struct S {}\n";
        let schema = schema_of(src);
        assert_eq!(schema.uses.len(), 1);
        assert_eq!(
            schema
                .items
                .iter()
                .map(|item| item.name().0)
                .collect::<Vec<_>>(),
            ["A", "B", "C", "S"]
        );

        let src = "namespace a\nstruct S { type T = u8 const C: u8 = 1 x: T, type: u8 }\n";
        let schema = schema_of(src);
        let s = item(&schema, "S");
        assert_eq!(fields(s), ["x: T", "type: u8"]);
        assert_eq!(
            s.body()
                .unwrap()
                .items
                .iter()
                .map(Item::keyword)
                .collect::<Vec<_>>(),
            ["type", "const"]
        );
    }

    #[test]
    fn a_semicolon_is_rejected() {
        let src = "namespace a;\nconst C: u8 = 1;\nstruct S {};\n";
        let (tokens, lex_errors) = lex(src);
        assert_eq!(
            lex_errors
                .iter()
                .map(|(error, span)| (*error, &src[span.clone()]))
                .collect::<Vec<_>>(),
            [(LexError::UnexpectedChar, ";"); 3]
        );
        // The lexer reports each `;` once; the parser never sees it.
        let (schema, errors) = parse(src, &tokens);
        assert_eq!(errors, []);
        assert_eq!(schema.expect("a schema").items.len(), 2);
    }

    fn attrs<'src>(item: &Item<'src>) -> Vec<String> {
        item.attrs().iter().map(Attribute::to_string).collect()
    }

    #[test]
    fn attributes_attach_to_the_declaration_below() {
        let src = concat!(
            "namespace a\n",
            "@visibility(Faction)\n",
            "@halo(false)\n",
            "@replicate(patch)\n",
            "struct S {\n",
            "    @replicate(ops(InventoryOp))\n",
            "    struct Inner {}\n",
            "    x: u8,\n",
            "    @timeout_ms(1000)\n",
            "    const C: u8 = 1\n",
            "    y: u8,\n",
            "}\n",
            "@large\n",
            "message(client -> sim) M {}\n",
            "@lock(Inventory, Gold)\n",
            "enum E { A }\n",
            "@lock(Inventory, Gold,)\n",
            "union U { A }\n",
            "@t\n",
            "type T = u8\n",
            "@x\n",
            "const K: u8 = 1\n",
            "struct Plain {}\n",
        );
        let schema = schema_of(src);
        let s = item(&schema, "S");
        assert_eq!(
            attrs(s),
            ["@visibility(Faction)", "@halo(false)", "@replicate(patch)"]
        );
        assert_eq!(fields(s), ["x: u8", "y: u8"]);
        let nested = &s.body().unwrap().items;
        assert_eq!(attrs(&nested[0]), ["@replicate(ops(InventoryOp))"]);
        assert_eq!(attrs(&nested[1]), ["@timeout_ms(1000)"]);
        assert_eq!(attrs(item(&schema, "M")), ["@large"]);
        assert_eq!(attrs(item(&schema, "E")), ["@lock(Inventory, Gold)"]);
        assert_eq!(attrs(item(&schema, "U")), ["@lock(Inventory, Gold)"]);
        assert_eq!(attrs(item(&schema, "T")), ["@t"]);
        assert_eq!(attrs(item(&schema, "K")), ["@x"]);
        assert!(item(&schema, "Plain").attrs().is_empty());

        let attr = &item(&schema, "S").attrs()[0];
        assert_eq!(&src[attr.span.clone()], "@visibility(Faction)");
        assert_eq!(&src[attr.name.1.clone()], "visibility");
        assert_eq!(&src[attr.args[0].span().clone()], "Faction");
    }

    #[test]
    fn attribute_arguments_are_literals_paths_or_calls() {
        let src =
            "namespace a\n@a(-1, 2.5, \"s\", true, false, B, c.d.E, f(g(1), h.I,)) struct S {}\n";
        let schema = schema_of(src);
        let [attr] = item(&schema, "S").attrs() else {
            panic!("expected one attribute");
        };
        assert_eq!(attr.name.0, "a");
        let [
            AttributeArg::Literal((Literal::Int(-1), _)),
            AttributeArg::Literal((Literal::Float(2.5), _)),
            AttributeArg::Literal((Literal::String(string), _)),
            AttributeArg::Literal((Literal::Bool(true), _)),
            AttributeArg::Literal((Literal::Bool(false), _)),
            AttributeArg::Path(b),
            AttributeArg::Path(e),
            AttributeArg::Call { path, args, span },
        ] = &attr.args[..]
        else {
            panic!("unexpected arguments: {:?}", attr.args);
        };
        assert_eq!(string, "s");
        assert_eq!(b.to_string(), "B");
        assert_eq!(e.to_string(), "c.d.E");
        assert_eq!(path.to_string(), "f");
        assert_eq!(&src[span.clone()], "f(g(1), h.I,)");
        let [
            AttributeArg::Call {
                path: g, args: one, ..
            },
            AttributeArg::Path(i),
        ] = &args[..]
        else {
            panic!("unexpected arguments: {args:?}");
        };
        assert_eq!(g.to_string(), "g");
        assert!(matches!(
            one[..],
            [AttributeArg::Literal((Literal::Int(1), _))]
        ));
        assert_eq!(i.to_string(), "h.I");
        assert_eq!(
            attr.to_string(),
            "@a(-1, 2.5, \"s\", true, false, B, c.d.E, f(g(1), h.I))"
        );
    }

    #[test]
    fn attribute_arguments_take_durations_and_timestamps() {
        let src = "namespace a\n@timeout(1000ms) @since(2026-01-01T00:00:00.500Z) struct S {}\n";
        let schema = schema_of(src);
        let [timeout, since] = item(&schema, "S").attrs() else {
            panic!("expected two attributes");
        };
        let [AttributeArg::Literal((Literal::Duration(1000, DurationUnit::Ms), span))] =
            &timeout.args[..]
        else {
            panic!("unexpected arguments: {:?}", timeout.args);
        };
        assert_eq!(&src[span.clone()], "1000ms");
        assert!(matches!(
            since.args[..],
            [AttributeArg::Literal((
                Literal::Timestamp(1_767_225_600_500_000),
                _
            ))]
        ));
        assert_eq!(timeout.to_string(), "@timeout(1000ms)");
        assert_eq!(since.to_string(), "@since(2026-01-01T00:00:00.500Z)");
    }

    #[test]
    fn line_breaks_carry_no_meaning_around_attributes() {
        let src =
            "namespace a @a @b(1) struct S {} @c\n(\n2\n)\nstruct T { @d enum E { A } x: u8 }\n";
        let schema = schema_of(src);
        assert_eq!(attrs(item(&schema, "S")), ["@a", "@b(1)"]);
        assert_eq!(attrs(item(&schema, "T")), ["@c(2)"]);
        assert_eq!(attrs(&item(&schema, "T").body().unwrap().items[0]), ["@d"]);
    }

    #[test]
    fn malformed_attributes_are_reported() {
        let error = |src: &str| {
            let errors = errors_of(src);
            assert_eq!(errors.len(), 1, "{errors:?}");
            (
                src[errors[0].span.clone()].to_string(),
                errors[0].message.clone(),
            )
        };
        assert_eq!(
            error("namespace a\n@\nstruct S {}\n"),
            (
                "struct".into(),
                "found 'struct' expected attribute name".into()
            )
        );
        assert_eq!(
            error("namespace a\n@1 struct S {}\n"),
            ("1".into(), "found '1' expected attribute name".into())
        );
        assert_eq!(
            error("namespace a\n@a(\n"),
            (
                "".into(),
                "found end of input expected argument, or ')'".into()
            )
        );
        assert_eq!(
            error("namespace a\nstruct S {}\n@a.b struct T {}\n"),
            (".".into(), "found '.' expected '(', or declaration".into())
        );
        assert_eq!(
            error("namespace a\n@a(1 2) struct S {}\n"),
            ("2".into(), "found '2' expected ',', or ')'".into())
        );
        assert_eq!(
            error("namespace a\n@a(,) struct S {}\n"),
            (",".into(), "found ',' expected argument, or ')'".into())
        );
        assert_eq!(
            error("namespace a\n@a() struct S {}\n"),
            (
                "()".into(),
                "empty parentheses; write `@a` without them".into()
            )
        );
        assert_eq!(
            error("namespace a\n@a(ops()) struct S {}\n"),
            (
                "()".into(),
                "empty parentheses; write `ops` without them".into()
            )
        );
    }

    #[test]
    fn an_attribute_must_go_above_a_declaration() {
        let error = |src: &str| {
            let errors = errors_of(src);
            assert_eq!(errors.len(), 1, "{errors:?}");
            (
                src[errors[0].span.clone()].to_string(),
                errors[0].message.clone(),
            )
        };
        let dangling = "an attribute must go above a declaration".to_string();
        assert_eq!(error("namespace a\n@a"), ("@a".into(), dangling.clone()));
        assert_eq!(
            error("namespace a\nstruct S {}\n@a @b(1)\n"),
            ("@a @b(1)".into(), dangling.clone())
        );
        assert_eq!(
            error("namespace a\nstruct S { x: u8, @a }\n"),
            ("@a".into(), dangling.clone())
        );
        assert_eq!(
            error("namespace a\nstruct S { @a }\nstruct T {}\n"),
            ("@a".into(), dangling)
        );
    }

    #[test]
    fn attributes_are_only_allowed_on_declarations() {
        let error = |src: &str| {
            let (tokens, _) = lex(src);
            let (schema, errors) = parse(src, &tokens);
            assert!(schema.is_some(), "parsing goes on");
            assert_eq!(errors.len(), 1, "{errors:?}");
            (
                src[errors[0].span.clone()].to_string(),
                errors[0].message.clone(),
            )
        };
        assert_eq!(
            error("@a @b namespace a\n"),
            (
                "@a @b".into(),
                "attributes are not allowed on `namespace`".into()
            )
        );
        assert_eq!(
            error("namespace a\n@a use b\nstruct S {}\n"),
            ("@a".into(), "attributes are not allowed on `use`".into())
        );
        for body in ["{ @a x: u8, y: u8 }", "{ x: u8, @a y: u8 }"] {
            assert_eq!(
                error(&format!("namespace a\nstruct S {body}\n")),
                ("@a".into(), "attributes are not allowed on fields".into())
            );
        }
        assert_eq!(
            error("namespace a\nunion U { A { @a x: u8 } }\n"),
            ("@a".into(), "attributes are not allowed on fields".into())
        );
        assert_eq!(
            error("namespace a\nenum E { A, @a(1) B }\n"),
            (
                "@a(1)".into(),
                "attributes are not allowed on enum variants".into()
            )
        );
        assert_eq!(
            error("namespace a\nunion U { @a A(u8) }\n"),
            (
                "@a".into(),
                "attributes are not allowed on union variants".into()
            )
        );
    }

    #[test]
    fn stray_tokens_after_a_declaration_are_reported() {
        assert_eq!(
            errors_of("namespace a\nconst A: i32 = 1 x\nstruct S {}\n")[0].message,
            "found 'x' expected declaration"
        );
    }

    #[test]
    fn imports_and_qualified_types_parse() {
        let schema = schema_of(EXAMPLE2);
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
        let src = "namespace a\nstruct S {\n    m: map<u32, vec<Position>>,\n}\n";
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
            let src = format!("namespace a\nstruct S {body}\n");
            assert_eq!(fields(item(&schema_of(&src), "S")), ["a: i32", "b: i32"]);
        }
        assert_eq!(
            errors_of("namespace a\nstruct S { a: i32 b: i32 }\n")[0].message,
            "found 'b' expected '[', ',', or '}'"
        );
    }

    #[test]
    fn malformed_types_are_reported() {
        assert_eq!(
            errors_of("namespace a\nstruct S { v: vec }\n")[0].message,
            "found '}' expected '<'"
        );
        assert_eq!(
            errors_of("namespace a\nstruct S { o: option }\n")[0].message,
            "found '}' expected '<'"
        );
        assert_eq!(
            errors_of("namespace a\nstruct S { v: array<i32, -1> }\n")[0].message,
            "`-1` is not a valid array length"
        );
        assert_eq!(
            errors_of("namespace a\nenum E: Color { A }\n")[0].message,
            "`Color` is not a builtin type"
        );
    }

    #[test]
    fn parsing_resumes_at_the_next_declaration() {
        let src = "namespace a\nstruct Bad { x: }\nstruct Good { y: i32 }\n";
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
