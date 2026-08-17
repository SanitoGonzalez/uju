pub mod ast;
pub mod codegen;
pub mod diag;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;

use codegen::{Backend, GeneratedFile};
use diag::{Diagnostic, Diagnostics, Severity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub text: String,
}

impl Source {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }
}

pub fn parse(file: usize, src: &str) -> Result<ast::Schema, Vec<Diagnostic>> {
    let tokens = lexer::lex(src).map_err(|spans| {
        spans
            .into_iter()
            .map(|span| Diagnostic {
                severity: Severity::Error,
                file,
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
                file,
                message: error.reason().to_string(),
                span: *error.span(),
                notes: Vec::new(),
            })
            .collect()
    })
}

pub fn compile(sources: &[Source]) -> Result<ir::Schema, Vec<Diagnostic>> {
    let mut files = Vec::with_capacity(sources.len());
    let mut errors = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        match parse(index, &source.text) {
            Ok(schema) => files.push(schema),
            Err(diags) => errors.extend(diags),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut diags = Diagnostics::new();
    let result =
        resolve::resolve(&files, &mut diags).and_then(|table| lower::lower(&table, &mut diags));
    match result {
        Some(schema) if !diags.has_errors() => Ok(schema),
        _ => Err(diags.into_vec()),
    }
}

pub fn compile_one(src: &str) -> Result<ir::Schema, Vec<Diagnostic>> {
    compile(&[Source::new("<input>", src)])
}

pub fn generate(
    sources: &[Source],
    backend: &dyn Backend,
) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let schema = compile(sources)?;
    Ok(backend.emit(&schema))
}

pub fn render(sources: &[Source], diagnostics: &[Diagnostic]) -> String {
    use core::fmt::Write as _;

    let mut out = String::new();
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let (name, line, column) = match sources.get(diagnostic.file) {
            Some(source) => {
                let start = diagnostic.span.start.min(source.text.len());
                let line = source.text[..start].lines().count().max(1);
                let column = source.text[..start]
                    .rsplit_once('\n')
                    .map(|(_, last)| last.len())
                    .unwrap_or(start)
                    + 1;
                (source.name.as_str(), line, column)
            }
            None => ("<unknown>", 0, 0),
        };
        let _ = writeln!(
            out,
            "{name}:{line}:{column}: {severity}: {}",
            diagnostic.message
        );
        for note in &diagnostic.notes {
            let _ = writeln!(out, "  note: {note}");
        }
    }
    out
}
