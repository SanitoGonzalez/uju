use core::fmt;

use logos::Logos;

use crate::ast::Prim;

pub type Span = core::ops::Range<usize>;

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
pub enum Token<'src> {
    #[token("namespace")]
    Namespace,

    #[token("use")]
    Use,

    #[token("const")]
    Const,

    #[token("enum")]
    Enum,

    #[token("struct")]
    Struct,

    #[token("component")]
    Component,

    #[token("message")]
    Message,

    #[token("vec")]
    Vec,

    #[token("set")]
    Set,

    #[token("map")]
    Map,

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
    #[token("timestamp", |_| Prim::Timestamp)]
    #[token("interval", |_| Prim::Interval)]
    #[token("entity", |_| Prim::Entity)]
    #[token("uentity", |_| Prim::UEntity)]
    #[token("string", |_| Prim::String)]
    #[token("bytes", |_| Prim::Bytes)]
    Prim(Prim),

    #[token("{")]
    BraceOpen,

    #[token("}")]
    BraceClose,

    #[token("<")]
    Lt,

    #[token(">")]
    Gt,

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

    #[token("->")]
    Arrow,

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

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Namespace => f.write_str("namespace"),
            Token::Use => f.write_str("use"),
            Token::Const => f.write_str("const"),
            Token::Enum => f.write_str("enum"),
            Token::Struct => f.write_str("struct"),
            Token::Component => f.write_str("component"),
            Token::Message => f.write_str("message"),
            Token::Vec => f.write_str("vec"),
            Token::Set => f.write_str("set"),
            Token::Map => f.write_str("map"),
            Token::Prim(p) => f.write_str(p.name()),
            Token::BraceOpen => f.write_str("{"),
            Token::BraceClose => f.write_str("}"),
            Token::Lt => f.write_str("<"),
            Token::Gt => f.write_str(">"),
            Token::Colon => f.write_str(":"),
            Token::Semicolon => f.write_str(";"),
            Token::Comma => f.write_str(","),
            Token::Dot => f.write_str("."),
            Token::Equal => f.write_str("="),
            Token::Question => f.write_str("?"),
            Token::Arrow => f.write_str("->"),
            Token::Ident(s) => f.write_str(s),
            Token::Bool(b) => write!(f, "{b}"),
            Token::Int(x) => write!(f, "{x}"),
            Token::Float(x) => write!(f, "{x}"),
            Token::Str(s) => f.write_str(s),
        }
    }
}

pub fn lex(src: &str) -> Result<Vec<(Token<'_>, Span)>, Vec<Span>> {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    for (result, span) in Token::lexer(src).spanned() {
        match result {
            Ok(token) => tokens.push((token, span)),
            Err(()) => errors.push(span),
        }
    }
    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}
