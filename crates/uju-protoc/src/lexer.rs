use std::fmt;

use logos::Logos;

use crate::parser::ast::DurationUnit;

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

    #[token("const")]
    Const,

    #[token("{")]
    BraceOpen,

    #[token("}")]
    BraceClose,

    #[token("[")]
    BracketOpen,

    #[token("]")]
    BracketClose,

    #[token("(")]
    ParenOpen,

    #[token(")")]
    ParenClose,

    #[token("<")]
    Lt,

    #[token(">")]
    Gt,

    #[token(":")]
    Colon,

    #[token(",")]
    Comma,

    #[token(".")]
    Dot,

    #[token("=")]
    Equal,

    #[token("@")]
    At,

    #[token("|")]
    Pipe,

    #[token("->")]
    Arrow,

    #[token("<->")]
    BiArrow,

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice())]
    Ident(&'src str),

    #[regex(r"-?[0-9]+", int)]
    #[regex(
        r"-?[0-9]+(\.[0-9]+)?[A-Za-z_][0-9A-Za-z_]*",
        malformed_number,
        priority = 3
    )]
    Int(i128),

    /// `30s`: an integer count and its unit, written without a space.
    #[regex(r"-?[0-9]+(us|ms|s|m|h)", duration, priority = 4)]
    Duration((i128, DurationUnit)),

    /// `2026-11-07T12:00:00Z`, with an optional 3- or 6-digit fraction of a
    /// second: microseconds since the Unix epoch.
    #[regex(
        r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]{3}|\.[0-9]{6})?Z",
        timestamp,
        priority = 4
    )]
    // Anything else that starts like a date, so that it is reported once
    // rather than as a run of numbers.
    #[regex(
        r"[0-9]+-[0-9]+-[0-9]+([A-Za-z][0-9A-Za-z:.+-]*)?",
        malformed_timestamp,
        priority = 3
    )]
    Timestamp(u64),

    #[regex(r"-?[0-9]+\.[0-9]+", float)]
    Float(f64),

    /// A string literal, with its escapes resolved.
    #[regex(r#""([^"\\\n]|\\[^\n])*""#, string)]
    #[regex(r#""([^"\\\n]|\\[^\n])*"#, unterminated_string)]
    Str(String),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LexError {
    #[default]
    UnexpectedChar,
    IntOverflow,
    FloatOverflow,
    UnterminatedComment,
    MalformedNumber,
    UnterminatedString,
    InvalidEscape,
    /// An integer followed by letters that are not a duration unit: `5d`.
    UnknownDurationUnit,
    /// More than one unit in a duration: `1h30m`.
    CompoundDuration,
    /// Starts like a date but is not `YYYY-MM-DDTHH:MM:SS[.fff[fff]]Z`.
    MalformedTimestamp,
    /// Well formed, but names a date or time that does not exist.
    InvalidTimestamp,
    TimestampBeforeEpoch,
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

/// Resolve the escapes of a string literal: `\"`, `\\`, `\n`, `\r`, `\t`,
/// `\0`, and `\u{...}` with one to six hex digits naming a Unicode scalar
/// value.
fn string<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<String> {
    let slice = lex.slice();
    let mut chars = slice[1..slice.len() - 1].chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        out.push(match chars.next() {
            Some('"') => '"',
            Some('\\') => '\\',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('0') => '\0',
            Some('u') => unicode_escape(&mut chars).ok_or(LexError::InvalidEscape)?,
            _ => return Err(LexError::InvalidEscape),
        });
    }
    Ok(out)
}

/// The scalar value of `{...}` after `\u`.
fn unicode_escape(chars: &mut std::str::Chars<'_>) -> Option<char> {
    if chars.next()? != '{' {
        return None;
    }
    let mut digits = String::new();
    loop {
        match chars.next()? {
            '}' => break,
            ch => digits.push(ch),
        }
    }
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    char::from_u32(u32::from_str_radix(&digits, 16).ok()?)
}

fn unterminated_string<'src>(_: &mut logos::Lexer<'src, Token<'src>>) -> Callback<String> {
    Err(LexError::UnterminatedString)
}

/// Tell an integer with a bad unit, or with several, from other malformed
/// numbers.
fn malformed_number<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<i128> {
    let slice = lex.slice();
    let rest = slice
        .strip_prefix('-')
        .unwrap_or(slice)
        .trim_start_matches(|ch: char| ch.is_ascii_digit());
    if rest.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(LexError::UnknownDurationUnit);
    }
    // `1h30m`: units with counts between them.
    let mut units = rest
        .split(|ch: char| ch.is_ascii_digit())
        .filter(|unit| !unit.is_empty());
    let ends_in_unit = rest.ends_with(|ch: char| ch.is_ascii_alphabetic());
    if ends_in_unit && units.all(|unit| DurationUnit::from_suffix(unit).is_some()) {
        return Err(LexError::CompoundDuration);
    }
    Err(LexError::MalformedNumber)
}

fn duration<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<(i128, DurationUnit)> {
    let slice = lex.slice();
    let split = slice
        .trim_end_matches(|ch: char| ch.is_ascii_alphabetic())
        .len();
    let unit = DurationUnit::from_suffix(&slice[split..]).expect("the regex admits only units");
    let count = slice[..split].parse().map_err(|_| LexError::IntOverflow)?;
    Ok((count, unit))
}

/// Validate a timestamp's date and time and convert it to microseconds since
/// the Unix epoch.
fn timestamp<'src>(lex: &mut logos::Lexer<'src, Token<'src>>) -> Callback<u64> {
    let slice = lex.slice();
    // The regex fixes every field's position and digit count.
    let field = |range: std::ops::Range<usize>| -> u64 {
        slice[range].parse().expect("the regex admits only digits")
    };
    let (year, month, day) = (field(0..4), field(5..7), field(8..10));
    let (hour, minute, second) = (field(11..13), field(14..16), field(17..19));
    let fraction = match &slice[19..slice.len() - 1] {
        "" => 0,
        digits => {
            let value: u64 = digits[1..].parse().expect("the regex admits only digits");
            if digits.len() == 4 {
                value * 1_000
            } else {
                value
            }
        }
    };

    let valid = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour <= 23
        && minute <= 59
        && second <= 59;
    if !valid {
        return Err(LexError::InvalidTimestamp);
    }
    if year < 1970 {
        return Err(LexError::TimestampBeforeEpoch);
    }

    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Ok(seconds * 1_000_000 + fraction)
}

fn malformed_timestamp<'src>(_: &mut logos::Lexer<'src, Token<'src>>) -> Callback<u64> {
    Err(LexError::MalformedTimestamp)
}

fn is_leap_year(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a date in the proleptic Gregorian calendar, on or
/// after it.
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    // Count from 0000-03-01 so that the leap day ends each year.
    let year = if month <= 2 { year - 1 } else { year };
    let (era, year_of_era) = (year / 400, year % 400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` after 1970-01-01, as `(year, month, day)`.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let (era, day_of_era) = (days / 146_097, days % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    (year, month, day)
}

/// Microseconds since the Unix epoch, printed as RFC 3339 in UTC: with no
/// fraction when it is zero, else with 3 digits if they suffice, else 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rfc3339(pub u64);

impl fmt::Display for Rfc3339 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = self.0 % 1_000_000;
        let seconds = self.0 / 1_000_000;
        let (year, month, day) = civil_from_days(seconds / 86_400);
        let time = seconds % 86_400;
        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            time / 3_600,
            time / 60 % 60,
            time % 60
        )?;
        match micros {
            0 => {}
            _ if micros.is_multiple_of(1_000) => write!(f, ".{:03}", micros / 1_000)?,
            _ => write!(f, ".{micros:06}")?,
        }
        f.write_str("Z")
    }
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
            Token::Const => f.write_str("const"),
            Token::BraceOpen => f.write_str("{"),
            Token::BraceClose => f.write_str("}"),
            Token::BracketOpen => f.write_str("["),
            Token::BracketClose => f.write_str("]"),
            Token::ParenOpen => f.write_str("("),
            Token::ParenClose => f.write_str(")"),
            Token::Lt => f.write_str("<"),
            Token::Gt => f.write_str(">"),
            Token::Colon => f.write_str(":"),
            Token::Comma => f.write_str(","),
            Token::Dot => f.write_str("."),
            Token::Equal => f.write_str("="),
            Token::At => f.write_str("@"),
            Token::Pipe => f.write_str("|"),
            Token::Arrow => f.write_str("->"),
            Token::BiArrow => f.write_str("<->"),
            Token::Ident(s) => f.write_str(s),
            Token::Int(x) => write!(f, "{x}"),
            Token::Float(x) => write!(f, "{x}"),
            Token::Str(x) => write!(f, "{x:?}"),
            Token::Duration((count, unit)) => write!(f, "{count}{unit}"),
            Token::Timestamp(micros) => write!(f, "{}", Rfc3339(*micros)),
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
            LexError::UnterminatedString => f.write_str("unterminated string literal"),
            LexError::InvalidEscape => f.write_str("invalid escape in string literal"),
            LexError::UnknownDurationUnit => {
                f.write_str("unknown duration unit; use `us`, `ms`, `s`, `m` or `h`")
            }
            LexError::CompoundDuration => {
                f.write_str("a duration literal takes a single unit, such as `90m`")
            }
            LexError::MalformedTimestamp => f.write_str(
                "malformed timestamp literal; expected `YYYY-MM-DDTHH:MM:SSZ`, \
                 optionally with 3 or 6 digits of fraction before the `Z`",
            ),
            LexError::InvalidTimestamp => {
                f.write_str("timestamp literal names a date or time that does not exist")
            }
            LexError::TimestampBeforeEpoch => {
                f.write_str("timestamp literal is before `1970-01-01T00:00:00Z`")
            }
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
            tokens("namespace use enum struct message const"),
            [
                Token::Namespace,
                Token::Use,
                Token::Enum,
                Token::Struct,
                Token::Message,
                Token::Const,
            ]
        );
        assert_eq!(
            tokens("{ } [ ] ( ) < > : , . = @ | -> <->"),
            [
                Token::BraceOpen,
                Token::BraceClose,
                Token::BracketOpen,
                Token::BracketClose,
                Token::ParenOpen,
                Token::ParenClose,
                Token::Lt,
                Token::Gt,
                Token::Colon,
                Token::Comma,
                Token::Dot,
                Token::Equal,
                Token::At,
                Token::Pipe,
                Token::Arrow,
                Token::BiArrow,
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
    fn contextual_keywords_are_idents() {
        // The parser treats them as keywords only where a declaration
        // starts, so `type: u8` stays a field.
        assert_eq!(
            tokens("type union"),
            [Token::Ident("type"), Token::Ident("union")]
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
    fn string_literals_resolve_their_escapes() {
        assert_eq!(
            tokens(r#""" "hi" "a\"b\\c" "\n\r\t\0" "\u{41}\u{1F600}" "héllo""#),
            [
                Token::Str(String::new()),
                Token::Str("hi".into()),
                Token::Str("a\"b\\c".into()),
                Token::Str("\n\r\t\0".into()),
                Token::Str("A\u{1F600}".into()),
                Token::Str("héllo".into()),
            ]
        );
        // A comment marker inside a string is part of it.
        assert_eq!(tokens(r#""// /*""#), [Token::Str("// /*".into())]);
    }

    #[test]
    fn bad_string_literals_are_reported() {
        for src in [
            r#""\q""#,
            r#""\u41""#,
            r#""\u{}""#,
            r#""\u{1234567}""#,
            r#""\u{D800}""#,
            r#""\u{zz}""#,
        ] {
            assert_eq!(
                errors(src),
                [(LexError::InvalidEscape, 0..src.len())],
                "{src}"
            );
        }
        assert_eq!(errors("\"open\nA"), [(LexError::UnterminatedString, 0..5)]);
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
    fn message_endpoints() {
        assert_eq!(
            tokens("message(client|hub<a.P>->sim) A (sim<->sim)"),
            [
                Token::Message,
                Token::ParenOpen,
                Token::Ident("client"),
                Token::Pipe,
                Token::Ident("hub"),
                Token::Lt,
                Token::Ident("a"),
                Token::Dot,
                Token::Ident("P"),
                Token::Gt,
                Token::Arrow,
                Token::Ident("sim"),
                Token::ParenClose,
                Token::Ident("A"),
                Token::ParenOpen,
                Token::Ident("sim"),
                Token::BiArrow,
                Token::Ident("sim"),
                Token::ParenClose,
            ]
        );
    }

    #[test]
    fn attributes() {
        assert_eq!(
            tokens("@replicate(ops(a.Op)) @large"),
            [
                Token::At,
                Token::Ident("replicate"),
                Token::ParenOpen,
                Token::Ident("ops"),
                Token::ParenOpen,
                Token::Ident("a"),
                Token::Dot,
                Token::Ident("Op"),
                Token::ParenClose,
                Token::ParenClose,
                Token::At,
                Token::Ident("large"),
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

        assert_eq!(
            errors("i32? ~ B"),
            [
                (LexError::UnexpectedChar, 3..4),
                (LexError::UnexpectedChar, 5..6),
            ]
        );

        // Nor is `;`: declarations take no terminator.
        assert_eq!(errors("namespace a;"), [(LexError::UnexpectedChar, 11..12)]);
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
        for src in [
            "0x1f", "0b1010", "1_000", "1e5", "1.5e3", "1.5s", "-0.5ms", "1h30",
        ] {
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
    fn duration_literals_take_every_unit() {
        assert_eq!(
            tokens("250us 1000ms 30s 5m 2h 0s -1s 007ms"),
            [
                Token::Duration((250, DurationUnit::Us)),
                Token::Duration((1000, DurationUnit::Ms)),
                Token::Duration((30, DurationUnit::S)),
                Token::Duration((5, DurationUnit::M)),
                Token::Duration((2, DurationUnit::H)),
                Token::Duration((0, DurationUnit::S)),
                Token::Duration((-1, DurationUnit::S)),
                Token::Duration((7, DurationUnit::Ms)),
            ]
        );
        assert_eq!(
            tokens("1000"),
            [Token::Int(1000)],
            "a bare integer stays one"
        );
        assert_eq!(
            tokens("30s)"),
            [Token::Duration((30, DurationUnit::S)), Token::ParenClose]
        );
        assert_eq!(
            tokens("30 s"),
            [Token::Int(30), Token::Ident("s")],
            "the unit is attached"
        );
        let (spanned, _) = lex("x 1000ms");
        assert_eq!(spanned[1].1, 2..8);
        assert_eq!(spanned[1].0.to_string(), "1000ms");
    }

    #[test]
    fn bad_duration_literals_are_reported() {
        for src in ["5d", "10ns", "3sec", "30S", "-7z", "4d"] {
            assert_eq!(
                errors(src),
                [(LexError::UnknownDurationUnit, 0..src.len())],
                "{src}"
            );
        }
        for src in ["1h30m", "1m30s", "1s500ms"] {
            assert_eq!(
                errors(src),
                [(LexError::CompoundDuration, 0..src.len())],
                "{src}"
            );
        }
        assert_eq!(errors("1.5s"), [(LexError::MalformedNumber, 0..4)]);
        assert_eq!(
            errors("340282366920938463463374607431768211456s"),
            [(LexError::IntOverflow, 0..40)]
        );
    }

    #[test]
    fn timestamp_literals_count_microseconds_since_the_epoch() {
        assert_eq!(
            tokens(concat!(
                "1970-01-01T00:00:00Z ",
                "2026-11-07T12:00:00Z ",
                "2026-11-07T12:00:00.250Z ",
                "2026-11-07T12:00:00.250125Z ",
                "2024-02-29T00:00:00Z ",
                "2000-02-29T23:59:59Z ",
                "9999-12-31T23:59:59.999999Z",
            )),
            [
                Token::Timestamp(0),
                Token::Timestamp(1_794_052_800_000_000),
                Token::Timestamp(1_794_052_800_250_000),
                Token::Timestamp(1_794_052_800_250_125),
                Token::Timestamp(1_709_164_800_000_000),
                Token::Timestamp(951_868_799_000_000),
                Token::Timestamp(253_402_300_799_999_999),
            ]
        );
        let (spanned, _) = lex("x 2026-11-07T12:00:00.250Z)");
        assert_eq!(spanned[1].1, 2..26);
        assert_eq!(spanned[2].0, Token::ParenClose);
    }

    #[test]
    fn timestamps_print_canonically() {
        for (src, printed) in [
            ("1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z"),
            ("2026-11-07T12:00:00.000Z", "2026-11-07T12:00:00Z"),
            ("2026-11-07T12:00:00.000000Z", "2026-11-07T12:00:00Z"),
            ("2026-11-07T12:00:00.250000Z", "2026-11-07T12:00:00.250Z"),
            ("2026-11-07T12:00:00.250Z", "2026-11-07T12:00:00.250Z"),
            ("2026-11-07T12:00:00.000125Z", "2026-11-07T12:00:00.000125Z"),
            ("2024-02-29T23:59:59Z", "2024-02-29T23:59:59Z"),
            ("2100-03-01T00:00:00Z", "2100-03-01T00:00:00Z"),
            ("9999-12-31T23:59:59.999999Z", "9999-12-31T23:59:59.999999Z"),
        ] {
            assert_eq!(tokens(src)[0].to_string(), printed, "{src}");
        }
        // Every day of a leap and a common year round-trips.
        for day in 0..(366 + 365) {
            let days = 19_723 + day; // 2024-01-01.
            let printed = Rfc3339(days * 86_400_000_000).to_string();
            assert_eq!(tokens(&printed), [Token::Timestamp(days * 86_400_000_000)]);
        }
    }

    #[test]
    fn impossible_timestamps_are_reported() {
        for src in [
            "2023-02-29T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-00-01T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T23:60:00Z",
            "2026-01-01T23:59:60Z",
        ] {
            assert_eq!(
                errors(src),
                [(LexError::InvalidTimestamp, 0..src.len())],
                "{src}"
            );
        }
        for src in ["1969-12-31T23:59:59Z", "0000-01-01T00:00:00Z"] {
            assert_eq!(
                errors(src),
                [(LexError::TimestampBeforeEpoch, 0..src.len())],
                "{src}"
            );
        }
    }

    #[test]
    fn malformed_timestamps_are_reported_once() {
        for src in [
            "2026-11-07",
            "2026-1-07T12:00:00Z",
            "26-11-07T12:00:00Z",
            "2026-11-07T12:00Z",
            "2026-11-07T12:00:00",
            "2026-11-07t12:00:00z",
            "2026-11-07T12:00:00+02:00",
            "2026-11-07T12:00:00-02:00",
            "2026-11-07T12:00:00.2Z",
            "2026-11-07T12:00:00.25Z",
            "2026-11-07T12:00:00.2501Z",
            "2026-11-07T12:00:00.25012Z",
            "2026-11-07T12:00:00.2501250Z",
            "2026-11-07T12:00:00ZZ",
        ] {
            assert_eq!(
                errors(src),
                [(LexError::MalformedTimestamp, 0..src.len())],
                "{src}"
            );
        }
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
    fn examples_lex_cleanly() {
        for src in [
            include_str!("../tests/data/example1.uju"),
            include_str!("../tests/data/example2.uju"),
        ] {
            let (spanned, errors) = lex(src);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(spanned[0].0, Token::Namespace);
            assert!(spanned.iter().any(|(token, _)| *token == Token::Message));
        }
    }
}
