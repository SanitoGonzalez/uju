use std::fmt;

use logos::Logos;

pub type Span = logos::Span;
pub type Spanned<T> = (T, Span);

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(error = LexError)]
#[logos(skip r"[ \t\r\n\f]+")]
#[logos(skip(r"//[^\n]*", allow_greedy = true))]
#[logos(skip(r"/\*", callback = block_comment))]
pub enum Token<'src> {
    #[token("namespace")]
    Namespace,

    #[token("use")]
    Use,

    #[token("enum")]
    Enum,

    #[token("struct")]
    Struct,

    #[token("message")]
    Message,

    #[token("{")]
    BraceOpen,

    #[token("}")]
    BraceClose,

    #[token("[")]
    BracketOpen,

    #[token("]")]
    BracketClose,

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

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice())]
    Ident(&'src str),

    #[regex(r"-?[0-9]+", int)]
    #[regex(r"-?[0-9]+(\.[0-9]+)?[A-Za-z_][0-9A-Za-z_]*", malformed_number)]
    Int(i128),

    #[regex(r"-?[0-9]+\.[0-9]+", float)]
    Float(f64),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LexError {
    #[default]
    UnexpectedChar,
    IntOverflow,
    FloatOverflow,
    UnterminatedComment,
    MalformedNumber,
}

/// Tokenize `src`, returning every token that could be read along with every
/// error that was found, so that parsing can proceed on a broken source.
pub fn lex(src: &str) -> (Vec<Spanned<Token<'_>>>, Vec<Spanned<LexError>>) {
    let mut tokens = Vec::new();
    let mut errors: Vec<Spanned<LexError>> = Vec::new();

    for (result, span) in Token::lexer(src).spanned() {
        match result {
            Ok(token) => tokens.push((token, span)),
            Err(LexError::UnexpectedChar) => match errors.last_mut() {
                Some((LexError::UnexpectedChar, last)) if last.end == span.start => {
                    last.end = span.end;
                }
                _ => errors.push((LexError::UnexpectedChar, span)),
            },
            Err(error) => errors.push((error, span)),
        }
    }

    (tokens, errors)
}

type Callback<T> = Result<T, LexError>;

fn int<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<i128> {
    lex.slice().parse().map_err(|_| LexError::IntOverflow)
}

fn float<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<f64> {
    match lex.slice().parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => Err(LexError::FloatOverflow),
    }
}

fn malformed_number<'src>(_: &mut logos::Lexer<'src, Token<'src>>) -> Callback<i128> {
    Err(LexError::MalformedNumber)
}

/// Consume a `/* */` comment, which may nest, after its opening delimiter.
fn block_comment<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Result<(), LexError> {
    let rest = lex.remainder().as_bytes();
    let mut depth = 1usize;
    let mut offset = 0;

    while offset + 1 < rest.len() {
        match (rest[offset], rest[offset + 1]) {
            (b'/', b'*') => {
                depth += 1;
                offset += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                offset += 2;
                if depth == 0 {
                    lex.bump(offset);
                    return Ok(());
                }
            }
            _ => offset += 1,
        }
    }

    lex.bump(rest.len());
    Err(LexError::UnterminatedComment)
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Namespace => f.write_str("namespace"),
            Token::Use => f.write_str("use"),
            Token::Enum => f.write_str("enum"),
            Token::Struct => f.write_str("struct"),
            Token::Message => f.write_str("message"),
            Token::BraceOpen => f.write_str("{"),
            Token::BraceClose => f.write_str("}"),
            Token::BracketOpen => f.write_str("["),
            Token::BracketClose => f.write_str("]"),
            Token::Lt => f.write_str("<"),
            Token::Gt => f.write_str(">"),
            Token::Colon => f.write_str(":"),
            Token::Semicolon => f.write_str(";"),
            Token::Comma => f.write_str(","),
            Token::Dot => f.write_str("."),
            Token::Equal => f.write_str("="),
            Token::Ident(s) => f.write_str(s),
            Token::Int(x) => write!(f, "{x}"),
            Token::Float(x) => write!(f, "{x}"),
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexError::UnexpectedChar => f.write_str("unexpected character"),
            LexError::IntOverflow => f.write_str("integer literal out of range"),
            LexError::FloatOverflow => f.write_str("float literal out of range"),
            LexError::UnterminatedComment => f.write_str("unterminated block comment"),
            LexError::MalformedNumber => f.write_str("malformed number literal"),
        }
    }
}

impl std::error::Error for LexError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(src: &str) -> Vec<Token<'_>> {
        let (tokens, errors) = lex(src);
        assert!(errors.is_empty(), "unexpected lex errors: {errors:?}");
        tokens.into_iter().map(|(token, _)| token).collect()
    }

    fn errors(src: &str) -> Vec<Spanned<LexError>> {
        let (_, errors) = lex(src);
        assert!(!errors.is_empty(), "expected lex errors");
        errors
    }

    #[test]
    fn keywords_and_punctuation() {
        assert_eq!(
            tokens("namespace use enum struct message"),
            [
                Token::Namespace,
                Token::Use,
                Token::Enum,
                Token::Struct,
                Token::Message,
            ]
        );
        assert_eq!(
            tokens("{ } [ ] < > : ; , . ="),
            [
                Token::BraceOpen,
                Token::BraceClose,
                Token::BracketOpen,
                Token::BracketClose,
                Token::Lt,
                Token::Gt,
                Token::Colon,
                Token::Semicolon,
                Token::Comma,
                Token::Dot,
                Token::Equal,
            ]
        );
    }

    #[test]
    fn keyword_prefixes_are_idents() {
        assert_eq!(
            tokens("usery enumeration _struct message1"),
            [
                Token::Ident("usery"),
                Token::Ident("enumeration"),
                Token::Ident("_struct"),
                Token::Ident("message1"),
            ]
        );
    }

    #[test]
    fn type_names_stay_idents() {
        assert_eq!(
            tokens("vec<map<u32, string>>"),
            [
                Token::Ident("vec"),
                Token::Lt,
                Token::Ident("map"),
                Token::Lt,
                Token::Ident("u32"),
                Token::Comma,
                Token::Ident("string"),
                Token::Gt,
                Token::Gt,
            ]
        );
    }

    #[test]
    fn literals() {
        assert_eq!(
            tokens("0 42 -7 1.5 -0.25 true"),
            [
                Token::Int(0),
                Token::Int(42),
                Token::Int(-7),
                Token::Float(1.5),
                Token::Float(-0.25),
                Token::Ident("true"),
            ]
        );
    }

    #[test]
    fn int_literals_span_i64_and_u64() {
        assert_eq!(
            tokens("-9223372036854775808 18446744073709551615"),
            [Token::Int(i64::MIN as i128), Token::Int(u64::MAX as i128),]
        );
    }

    #[test]
    fn constraints() {
        assert_eq!(
            tokens("i32[min=-100, infinite]"),
            [
                Token::Ident("i32"),
                Token::BracketOpen,
                Token::Ident("min"),
                Token::Equal,
                Token::Int(-100),
                Token::Comma,
                Token::Ident("infinite"),
                Token::BracketClose,
            ]
        );
    }

    #[test]
    fn direction_attributes() {
        assert_eq!(
            tokens("message[in] A"),
            [
                Token::Message,
                Token::BracketOpen,
                Token::Ident("in"),
                Token::BracketClose,
                Token::Ident("A"),
            ]
        );
    }

    #[test]
    fn paths_are_not_floats() {
        assert_eq!(
            tokens("foo.bar.Position"),
            [
                Token::Ident("foo"),
                Token::Dot,
                Token::Ident("bar"),
                Token::Dot,
                Token::Ident("Position"),
            ]
        );
    }

    #[test]
    fn comments_and_whitespace_are_skipped() {
        let src = "// leading\r\nenum A { B = 1 } // trailing, no newline at eof";
        assert_eq!(
            tokens(src),
            [
                Token::Enum,
                Token::Ident("A"),
                Token::BraceOpen,
                Token::Ident("B"),
                Token::Equal,
                Token::Int(1),
                Token::BraceClose,
            ]
        );
    }

    #[test]
    fn spans_point_at_the_lexeme() {
        let src = "enum Color {}";
        let (spanned, _) = lex(src);
        assert_eq!(spanned[1], (Token::Ident("Color"), 5..10));
        assert_eq!(&src[spanned[2].1.clone()], "{");
    }

    #[test]
    fn unknown_characters_are_reported_as_one_run() {
        assert_eq!(errors("a $$$ b"), [(LexError::UnexpectedChar, 2..5)]);
        assert_eq!(
            errors("$ a $"),
            [
                (LexError::UnexpectedChar, 0..1),
                (LexError::UnexpectedChar, 4..5),
            ]
        );

        // `?` and `->` are no longer part of the language.
        assert_eq!(
            errors("i32? -> B"),
            [
                (LexError::UnexpectedChar, 3..4),
                (LexError::UnexpectedChar, 5..6),
            ]
        );
    }

    #[test]
    fn bad_literals_are_reported() {
        assert_eq!(
            errors("340282366920938463463374607431768211456"),
            [(LexError::IntOverflow, 0..39)]
        );
    }

    #[test]
    fn every_error_is_collected() {
        assert_eq!(
            errors("§ enum § A §").len(),
            3,
            "each bad character run should produce its own error"
        );
    }

    #[test]
    fn block_comments_are_skipped() {
        assert_eq!(
            tokens("/* leading */ enum /* /* nested */ */ A /* trailing */"),
            [Token::Enum, Token::Ident("A")]
        );
        assert_eq!(
            tokens("/* a\n * b\n */ A"),
            [Token::Ident("A")],
            "block comments span lines"
        );
        assert_eq!(tokens("A /*/ still open */"), [Token::Ident("A")]);
    }

    #[test]
    fn unterminated_block_comments_are_reported() {
        assert_eq!(errors("A /* oops"), [(LexError::UnterminatedComment, 2..9)]);
        assert_eq!(
            errors("/* /* only one closed */"),
            [(LexError::UnterminatedComment, 0..24)]
        );
    }

    #[test]
    fn unsupported_number_forms_are_reported() {
        for src in ["0x1f", "0b1010", "1_000", "1e5", "1.5e3", "-7z", "4d"] {
            assert_eq!(
                errors(src),
                [(LexError::MalformedNumber, 0..src.len())],
                "{src} should not lex as a number followed by an identifier"
            );
        }
        assert_eq!(
            tokens("0 42 -7 007 1.5 -0.25"),
            [
                Token::Int(0),
                Token::Int(42),
                Token::Int(-7),
                Token::Int(7),
                Token::Float(1.5),
                Token::Float(-0.25),
            ]
        );
    }

    #[test]
    fn tokens_survive_errors() {
        let (tokens, errors) = lex("enum A { B = $ 1 }");
        assert_eq!(
            tokens
                .into_iter()
                .map(|(token, _)| token)
                .collect::<Vec<_>>(),
            [
                Token::Enum,
                Token::Ident("A"),
                Token::BraceOpen,
                Token::Ident("B"),
                Token::Equal,
                Token::Int(1),
                Token::BraceClose,
            ]
        );
        assert_eq!(errors, [(LexError::UnexpectedChar, 13..14)]);
    }

    #[test]
    fn samples_lex_cleanly() {
        for src in [
            include_str!("../tests/data/sample1.uju"),
            include_str!("../tests/data/sample2.uju"),
        ] {
            let (spanned, errors) = lex(src);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(spanned[0].0, Token::Namespace);
            assert!(spanned.iter().any(|(token, _)| *token == Token::Message));
        }
    }
}
