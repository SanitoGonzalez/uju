use std::borrow::Cow;
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

    #[token("const")]
    Const,

    #[token("enum")]
    Enum,

    #[token("struct")]
    Struct,

    #[token("union")]
    Union,

    #[token("message")]
    Message,

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

    #[regex(r"-?[0-9]+", int)]
    #[regex(r"-?[0-9]+(\.[0-9]+)?[A-Za-z_][0-9A-Za-z_]*", malformed_number)]
    Int(i128),

    #[regex(r"-?[0-9]+\.[0-9]+", float)]
    Float(f64),

    #[regex(r#""([^"\\\n]|\\.)*""#, string)]
    #[regex(r#""([^"\\\n]|\\.)*"#, unterminated_string)]
    Str(&'src str),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LexError {
    #[default]
    UnexpectedChar,
    IntOverflow,
    FloatOverflow,
    UnterminatedString,
    InvalidEscape,
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

/// Decode the escape sequences of a string literal's contents, as carried by
/// [`Token::Str`].
pub fn unescape(contents: &str) -> Cow<'_, str> {
    if !contents.contains('\\') {
        return Cow::Borrowed(contents);
    }

    let mut out = String::with_capacity(contents.len());
    let mut chars = contents.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            // `\\` and `\"`; anything else is rejected while lexing
            Some(c) => out.push(c),
            None => {}
        }
    }
    Cow::Owned(out)
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

fn string<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<&'src str> {
    let slice = lex.slice();
    let contents = &slice[1..slice.len() - 1];

    let mut chars = contents.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && !matches!(chars.next(), Some('\\' | '"' | 'n' | 'r' | 't' | '0')) {
            return Err(LexError::InvalidEscape);
        }
    }

    Ok(contents)
}

fn unterminated_string<'src>(_: &mut logos::Lexer<'src, Token<'src>>) -> Callback<&'src str> {
    Err(LexError::UnterminatedString)
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
            Token::Const => f.write_str("const"),
            Token::Enum => f.write_str("enum"),
            Token::Struct => f.write_str("struct"),
            Token::Union => f.write_str("union"),
            Token::Message => f.write_str("message"),
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
            Token::Str(s) => write!(f, "\"{s}\""),
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexError::UnexpectedChar => f.write_str("unexpected character"),
            LexError::IntOverflow => f.write_str("integer literal out of range"),
            LexError::FloatOverflow => f.write_str("float literal out of range"),
            LexError::UnterminatedString => f.write_str("unterminated string literal"),
            LexError::InvalidEscape => f.write_str("unknown escape sequence"),
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
            tokens("namespace use const enum struct union message"),
            [
                Token::Namespace,
                Token::Use,
                Token::Const,
                Token::Enum,
                Token::Struct,
                Token::Union,
                Token::Message,
            ]
        );
        assert_eq!(
            tokens("{ } < > : ; , . = ? ->"),
            [
                Token::BraceOpen,
                Token::BraceClose,
                Token::Lt,
                Token::Gt,
                Token::Colon,
                Token::Semicolon,
                Token::Comma,
                Token::Dot,
                Token::Equal,
                Token::Question,
                Token::Arrow,
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
            tokens(r#"0 42 -7 1.5 -0.25 true false "hi""#),
            [
                Token::Int(0),
                Token::Int(42),
                Token::Int(-7),
                Token::Float(1.5),
                Token::Float(-0.25),
                Token::Bool(true),
                Token::Bool(false),
                Token::Str("hi"),
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
    fn arrow_is_not_a_negative_number() {
        assert_eq!(
            tokens("message A -> B"),
            [
                Token::Message,
                Token::Ident("A"),
                Token::Arrow,
                Token::Ident("B"),
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
        let src = "// leading\r\nconst A: i32 = 1; // trailing, no newline at eof";
        assert_eq!(
            tokens(src),
            [
                Token::Const,
                Token::Ident("A"),
                Token::Colon,
                Token::Ident("i32"),
                Token::Equal,
                Token::Int(1),
                Token::Semicolon,
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
    fn escapes_are_kept_raw_and_decoded_on_demand() {
        let [Token::Str(contents)] = tokens(r#""a\nb\\c\"d""#)[..] else {
            panic!("expected a single string token");
        };
        assert_eq!(contents, r#"a\nb\\c\"d"#);
        assert_eq!(unescape(contents), "a\nb\\c\"d");
        assert!(matches!(unescape("plain"), Cow::Borrowed("plain")));
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
    }

    #[test]
    fn bad_literals_are_reported() {
        assert_eq!(
            errors("340282366920938463463374607431768211456"),
            [(LexError::IntOverflow, 0..39)]
        );
        assert_eq!(errors(r#""oops"#), [(LexError::UnterminatedString, 0..5)]);
        assert_eq!(
            errors("\"a\nb\"").first().unwrap().0,
            LexError::UnterminatedString
        );
        assert_eq!(
            errors(r#""bad \q escape""#),
            [(LexError::InvalidEscape, 0..15)]
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
            tokens("/* leading */ const /* /* nested */ */ A /* trailing */"),
            [Token::Const, Token::Ident("A")]
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
        let (tokens, errors) = lex("const A: i32 = $ 1;");
        assert_eq!(
            tokens
                .into_iter()
                .map(|(token, _)| token)
                .collect::<Vec<_>>(),
            [
                Token::Const,
                Token::Ident("A"),
                Token::Colon,
                Token::Ident("i32"),
                Token::Equal,
                Token::Int(1),
                Token::Semicolon,
            ]
        );
        assert_eq!(errors, [(LexError::UnexpectedChar, 15..16)]);
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
