use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};

use crate::backend::Backend;
use crate::ir::*;

pub struct Rust;

impl Backend for Rust {
    fn generate(&self, schema: &ir::Schema) -> Result<Vec<GeneratedFile>, Error>;
}
