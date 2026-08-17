pub mod rust;

use std::path::PathBuf;

use crate::ir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

pub trait Backend {
    fn name(&self) -> &str;

    fn emit(&self, schema: &ir::Schema) -> Vec<GeneratedFile>;
}

pub fn backend(name: &str) -> Option<Box<dyn Backend>> {
    todo!()
}
