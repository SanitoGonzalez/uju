use chumsky::input::{Input, Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::{
    Const, Enum, Expr, Field, Ident, Item, Message, Path, Prim, Schema, Span, Spanned, Struct,
    StructKind, TypeRef, Variant,
};
use crate::lexer::{self, Token};

pub type ParseError<'src> = Rich<'src, Token<'src>, Span>;

pub trait TokenInput<'src>: ValueInput<'src, Token = Token<'src>, Span = Span> {}

impl<'src, I> TokenInput<'src> for I where I: ValueInput<'src, Token = Token<'src>, Span = Span> {}

pub fn parse<'src>(
    tokens: &'src [(Token<'src>, lexer::Span)],
) -> Result<Schema, Vec<ParseError<'src>>> {
    let eoi = tokens.last().map(|(_, span)| span.end).unwrap_or(0);
    let input = Stream::from_iter(
        tokens
            .iter()
            .map(|(token, span)| (token.clone(), Span::from(span.clone()))),
    )
    .map(Span::from(eoi..eoi), |x| x);
    schema().parse(input).into_result()
}

pub fn schema<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Schema, extra::Err<ParseError<'src>>> {
    namespace()
        .or_not()
        .then(use_decl().repeated().collect())
        .then(item().repeated().collect())
        .then_ignore(end())
        .map(|((namespace, uses), items)| Schema {
            namespace,
            uses,
            items,
        })
}

pub fn namespace<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Path, extra::Err<ParseError<'src>>> {
    just(Token::Namespace)
        .ignore_then(path())
        .then_ignore(just(Token::Semicolon))
}

pub fn use_decl<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Path, extra::Err<ParseError<'src>>> {
    just(Token::Use)
        .ignore_then(path())
        .then_ignore(just(Token::Semicolon))
}

pub fn item<'src, I: TokenInput<'src>>() -> impl Parser<'src, I, Item, extra::Err<ParseError<'src>>>
{
    choice((
        const_def().map(Item::Const),
        enum_def().map(Item::Enum),
        struct_def().map(Item::Struct),
        message_def().map(Item::Message),
    ))
}

pub fn const_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Const, extra::Err<ParseError<'src>>> {
    just(Token::Const)
        .ignore_then(ident())
        .then_ignore(just(Token::Colon))
        .then(ty())
        .then_ignore(just(Token::Equal))
        .then(expr())
        .then_ignore(just(Token::Semicolon))
        .map(|((name, ty), value)| Const { name, ty, value })
}

pub fn enum_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Enum, extra::Err<ParseError<'src>>> {
    just(Token::Enum)
        .ignore_then(ident())
        .then(just(Token::Colon).ignore_then(prim()).or_not())
        .then(
            variant()
                .then_ignore(just(Token::Comma))
                .repeated()
                .collect()
                .delimited_by(just(Token::BraceOpen), just(Token::BraceClose)),
        )
        .map(|((name, repr), variants)| Enum {
            name,
            repr,
            variants,
        })
}

pub fn variant<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Variant, extra::Err<ParseError<'src>>> {
    let value = select! { Token::Int(x) = e => Spanned::new(x, e.span()) };
    ident()
        .then(just(Token::Equal).ignore_then(value).or_not())
        .map(|(name, value)| Variant { name, value })
}

pub fn struct_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Struct, extra::Err<ParseError<'src>>> {
    choice((
        just(Token::Struct).to(StructKind::Struct),
        just(Token::Component).to(StructKind::Component),
    ))
    .then(ident())
    .then(fields())
    .map(|((kind, name), fields)| Struct { kind, name, fields })
}

pub fn message_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Message, extra::Err<ParseError<'src>>> {
    enum Entry {
        Item(Item),
        Field(Field),
    }

    let nested = choice((
        const_def().map(Item::Const),
        enum_def().map(Item::Enum),
        struct_def().map(Item::Struct),
    ));
    let entry = choice((
        nested.map(Entry::Item),
        field().then_ignore(just(Token::Comma)).map(Entry::Field),
    ));

    just(Token::Message)
        .ignore_then(ident())
        .then(just(Token::Arrow).ignore_then(path()).or_not())
        .then(
            entry
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just(Token::BraceOpen), just(Token::BraceClose)),
        )
        .map(|((name, returns), entries)| {
            let mut items = Vec::new();
            let mut fields = Vec::new();
            for entry in entries {
                match entry {
                    Entry::Item(item) => items.push(item),
                    Entry::Field(field) => fields.push(field),
                }
            }
            Message {
                name,
                returns,
                items,
                fields,
            }
        })
}

pub fn fields<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Vec<Field>, extra::Err<ParseError<'src>>> {
    field()
        .then_ignore(just(Token::Comma))
        .repeated()
        .collect()
        .delimited_by(just(Token::BraceOpen), just(Token::BraceClose))
}

pub fn field<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Field, extra::Err<ParseError<'src>>> {
    ident()
        .then_ignore(just(Token::Colon))
        .then(
            ty().then(just(Token::Question).or_not())
                .map_with(|(inner, opt), e| match opt {
                    Some(_) => Spanned::new(TypeRef::Optional(Box::new(inner)), e.span()),
                    None => inner,
                }),
        )
        .map(|(name, ty)| Field { name, ty })
}

pub fn ty<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Spanned<TypeRef>, extra::Err<ParseError<'src>>> {
    recursive(|ty| {
        let generic = |t| {
            just(t).ignore_then(
                ty.clone()
                    .delimited_by(just(Token::Lt), just(Token::Gt))
                    .map(Box::new),
            )
        };
        choice((
            prim().map(|p| TypeRef::Prim(p.node)),
            generic(Token::Vec).map(TypeRef::Vec),
            generic(Token::Set).map(TypeRef::Set),
            just(Token::Map)
                .ignore_then(
                    ty.clone()
                        .then_ignore(just(Token::Comma))
                        .then(ty.clone())
                        .delimited_by(just(Token::Lt), just(Token::Gt)),
                )
                .map(|(k, v)| TypeRef::Map(Box::new(k), Box::new(v))),
            path().map(TypeRef::Named),
        ))
        .map_with(|node, e| Spanned::new(node, e.span()))
    })
}

pub fn prim<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Spanned<Prim>, extra::Err<ParseError<'src>>> + Clone {
    select! { Token::Prim(p) = e => Spanned::new(p, e.span()) }
}

pub fn expr<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Spanned<Expr>, extra::Err<ParseError<'src>>> {
    let literal = select! {
        Token::Int(x) => Expr::Int(x),
        Token::Float(x) => Expr::Float(x),
        Token::Bool(b) => Expr::Bool(b),
        Token::Str(s) => Expr::Str(unescape(s)),
    };
    literal
        .or(path().map(Expr::Path))
        .map_with(|node, e| Spanned::new(node, e.span()))
}

pub fn path<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Path, extra::Err<ParseError<'src>>> + Clone {
    ident()
        .separated_by(just(Token::Dot))
        .at_least(1)
        .collect()
        .map(Path)
}

pub fn ident<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Ident, extra::Err<ParseError<'src>>> + Clone {
    select! { Token::Ident(s) = e => Spanned::new(s.to_string(), e.span()) }
}

fn unescape(quoted: &str) -> String {
    let inner = &quoted[1..quoted.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}
