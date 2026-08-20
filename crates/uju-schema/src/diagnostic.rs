use std::fmt;

use crate::lexer::Span;

/// Index of a source file in the set passed to [`crate::compile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId(pub usize);

/// A problem found while compiling, pointing back at the source it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub source: SourceId,
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn new(source: SourceId, span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            source,
            span,
            message: message.into(),
        }
    }

    /// Render as `name:line:column: message`, locating the span in the text
    /// this diagnostic was produced from.
    pub fn render(&self, name: &str, src: &str) -> String {
        let start = self.span.start.min(src.len());
        let line = src[..start].matches('\n').count() + 1;
        let column = start - src[..start].rfind('\n').map_or(0, |index| index + 1) + 1;
        format!("{name}:{line}:{column}: {}", self.message)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_with_line_and_column() {
        let src = "namespace a;\nstruct S {}\n";
        let diagnostic = Diagnostic::new(SourceId(0), 13..19, "boom");
        assert_eq!(diagnostic.render("x.uju", src), "x.uju:2:1: boom");

        let first = Diagnostic::new(SourceId(0), 10..11, "boom");
        assert_eq!(first.render("x.uju", src), "x.uju:1:11: boom");
    }

    #[test]
    fn spans_past_the_end_are_clamped() {
        let diagnostic = Diagnostic::new(SourceId(0), 100..101, "eof");
        assert_eq!(diagnostic.render("x.uju", "ab"), "x.uju:1:3: eof");
    }
}
