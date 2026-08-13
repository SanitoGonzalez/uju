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
        todo!()
    }

    pub fn error(&mut self, span: Span, message: impl Into<String>) {
        todo!()
    }

    pub fn warning(&mut self, span: Span, message: impl Into<String>) {
        todo!()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        todo!()
    }

    pub fn has_errors(&self) -> bool {
        todo!()
    }

    pub fn entries(&self) -> &[Diagnostic] {
        todo!()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        todo!()
    }
}
