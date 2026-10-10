pub mod backend;
pub mod diagnostic;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod resolve;

use crate::diagnostic::{Diagnostic, SourceId};

/// Compile schema sources into the IR.
pub fn compile(sources: &[&str]) -> Result<ir::Schema, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut schemas = Vec::with_capacity(sources.len());

    for (index, src) in sources.iter().enumerate() {
        let source = SourceId(index);
        let (tokens, lex_errors) = lexer::lex(src);
        diagnostics.extend(
            lex_errors
                .into_iter()
                .map(|(error, span)| Diagnostic::new(source, span, error.to_string())),
        );
        let (schema, parse_errors) = parser::parse(src, &tokens);
        diagnostics.extend(
            parse_errors
                .into_iter()
                .map(|error| Diagnostic::new(source, error.span, error.message)),
        );
        schemas.push(schema);
    }

    let schemas: Vec<_> = schemas.into_iter().flatten().collect();
    if !diagnostics.is_empty() || schemas.len() != sources.len() {
        return Err(diagnostics);
    }

    ir::lower(&schemas)
}
