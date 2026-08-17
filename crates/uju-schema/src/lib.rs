pub mod ast;
pub mod codegen;
pub mod diag;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;

use codegen::{Backend, GeneratedFile};
use diag::{Diagnostic, Severity};

pub fn parse(src: &str) -> Result<ast::Schema, Vec<Diagnostic>> {
    let tokens = lexer::lex(src).map_err(|spans| {
        spans
            .into_iter()
            .map(|span| Diagnostic {
                severity: Severity::Error,
                message: format!("unrecognized token `{}`", &src[span.clone()]),
                span: span.into(),
                notes: Vec::new(),
            })
            .collect::<Vec<_>>()
    })?;
    parser::parse(&tokens).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Diagnostic {
                severity: Severity::Error,
                message: error.reason().to_string(),
                span: *error.span(),
                notes: Vec::new(),
            })
            .collect()
    })
}

pub fn compile(src: &str) -> Result<ir::Schema, Vec<Diagnostic>> {
    let schema = parse(src)?;
    let mut diags = diag::Diagnostics::new();
    let result = resolve::resolve(&schema, &mut diags)
        .and_then(|table| lower::lower(&schema, &table, &mut diags));
    match result {
        Some(schema) if !diags.has_errors() => Ok(schema),
        _ => Err(diags.into_vec()),
    }
}

pub fn generate(src: &str, backend: &dyn Backend) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let schema = compile(src)?;
    Ok(backend.emit(&schema))
}
