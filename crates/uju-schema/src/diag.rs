use crate::ast::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    entries: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn error(&mut self, span: Span, message: impl Into<String>) {
        self.push(Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
            notes: Vec::new(),
        });
    }

    pub fn warning(&mut self, span: Span, message: impl Into<String>) {
        self.push(Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            span,
            notes: Vec::new(),
        });
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.entries.push(diagnostic);
    }

    pub fn has_errors(&self) -> bool {
        self.entries.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn entries(&self) -> &[Diagnostic] {
        &self.entries
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.entries
    }
}
