use chumsky::input::ValueInput;
use chumsky::prelude::*;

use crate::ast::{
    Const, Enum, Expr, Field, Ident, Item, Path, Prim, Schema, Span, Spanned, Struct, TypeRef,
    Variant,
};
use crate::lexer::{self, Token};

pub type ParseError<'src> = Rich<'src, Token<'src>, Span>;

pub trait TokenInput<'src>: ValueInput<'src, Token = Token<'src>, Span = Span> {}

impl<'src, I> TokenInput<'src> for I where I: ValueInput<'src, Token = Token<'src>, Span = Span> {}

pub fn parse<'src>(
    tokens: &'src [(Token<'src>, lexer::Span)],
) -> Result<Schema, Vec<ParseError<'src>>> {
    todo!()
}

pub fn schema<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Schema, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn namespace<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Path, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn item<'src, I: TokenInput<'src>>() -> impl Parser<'src, I, Item, extra::Err<ParseError<'src>>>
{
    todo()
}

pub fn const_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Const, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn enum_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Enum, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn variant<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Variant, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn struct_def<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Struct, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn field<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Field, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn ty<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Spanned<TypeRef>, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn prim<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Spanned<Prim>, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn expr<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Spanned<Expr>, extra::Err<ParseError<'src>>> {
    todo()
}

pub fn path<'src, I: TokenInput<'src>>() -> impl Parser<'src, I, Path, extra::Err<ParseError<'src>>>
{
    todo()
}

pub fn ident<'src, I: TokenInput<'src>>()
-> impl Parser<'src, I, Ident, extra::Err<ParseError<'src>>> {
    todo()
}
