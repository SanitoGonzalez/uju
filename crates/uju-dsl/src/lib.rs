pub mod ast;
pub mod case;
pub mod codegen;
pub mod diag;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;

use codegen::{Backend, GeneratedFile};
use diag::Diagnostic;

pub fn compile(src: &str) -> Result<ir::Schema, Vec<Diagnostic>> {
    todo!()
}

pub fn generate(src: &str, backend: &dyn Backend) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    todo!()
}
