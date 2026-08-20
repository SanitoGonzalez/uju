pub mod rust;

use crate::backend::rust::Rust;

pub struct GeneratedFile {
    path: PathBuf,
    contents: String,
}

pub trait Backend {
    fn generate(&self, schema: &ir::Schema) -> Result<Vec<GeneratedFile>, Error>;
}

pub fn backend(name: &str) -> Option<Box<dyn Backend>> {
    match name {
        "rust" => Some(Box::new(Rust)),
        _ => None,
    }
}
