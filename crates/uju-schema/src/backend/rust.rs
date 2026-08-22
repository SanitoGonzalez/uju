use crate::backend::{Backend, Error, GeneratedFile};
use crate::ir;

pub struct Rust;

impl Backend for Rust {
    fn generate(&self, _schema: &ir::Schema) -> Result<Vec<GeneratedFile>, Error> {
        todo!()
    }
}
