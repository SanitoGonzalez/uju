pub mod backend;
pub mod case;
pub mod config;
pub mod diagnostic;
pub mod ir;
pub mod lexer;
mod lint;
pub mod parser;
pub mod resolve;

use crate::diagnostic::{Diagnostic, SourceId};

/// A compiled schema, with the warnings compiling it raised.
#[derive(Debug)]
pub struct Compiled {
    pub schema: ir::Schema,
    pub warnings: Vec<Diagnostic>,
}

/// Compile schema sources into the IR. On failure, the diagnostics hold
/// the errors along with any warnings raised before them.
pub fn compile(sources: &[&str]) -> Result<Compiled, Vec<Diagnostic>> {
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

    let mut warnings = Vec::new();
    for (index, schema) in schemas.iter().enumerate() {
        lint::naming(SourceId(index), schema, &mut warnings);
    }
    match ir::lower(&schemas) {
        Ok(schema) => Ok(Compiled { schema, warnings }),
        Err(mut errors) => {
            errors.extend(warnings);
            Err(errors)
        }
    }
}
