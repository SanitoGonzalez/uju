use logos::Logos;

use crate::ast::Prim;

pub type Span = core::ops::Range<usize>;

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
pub enum Token<'src> {
    #[token("namespace")]
    Namespace,

    #[token("const")]
    Const,

    #[token("enum")]
    Enum,

    #[token("struct")]
    Struct,

    #[token("i8", |_| Prim::I8)]
    #[token("i16", |_| Prim::I16)]
    #[token("i32", |_| Prim::I32)]
    #[token("i64", |_| Prim::I64)]
    #[token("u8", |_| Prim::U8)]
    #[token("u16", |_| Prim::U16)]
    #[token("u32", |_| Prim::U32)]
    #[token("u64", |_| Prim::U64)]
    #[token("f32", |_| Prim::F32)]
    #[token("f64", |_| Prim::F64)]
    #[token("bool", |_| Prim::Bool)]
    #[token("string", |_| Prim::String)]
    Prim(Prim),

    #[token("{")]
    BraceOpen,

    #[token("}")]
    BraceClose,

    #[token("[")]
    BracketOpen,

    #[token("]")]
    BracketClose,

    #[token(":")]
    Colon,

    #[token(";")]
    Semicolon,

    #[token(",")]
    Comma,

    #[token(".")]
    Dot,

    #[token("=")]
    Equal,

    #[token("?")]
    Question,

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice())]
    Ident(&'src str),

    #[token("true", |_| true)]
    #[token("false", |_| false)]
    Bool(bool),

    #[regex(r"-?[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    Int(i64),

    #[regex(r"-?[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    Float(f64),

    #[regex(r#""([^"\\]|\\.)*""#, |lex| lex.slice())]
    Str(&'src str),
}

pub fn lex(src: &str) -> Result<Vec<(Token<'_>, Span)>, Vec<Span>> {
    todo!()
}
