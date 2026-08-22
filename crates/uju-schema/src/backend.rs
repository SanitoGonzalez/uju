pub mod ir;
pub mod rust;

use std::fmt;
use std::path::PathBuf;

use crate::backend::rust::Rust;
use crate::ir::Schema;

/// Every name [`backend`] accepts, for listing in help and error messages.
pub const BACKENDS: &[&str] = &["rust"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

impl GeneratedFile {
    pub fn new(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        GeneratedFile {
            path: path.into(),
            contents: contents.into(),
        }
    }
}

/// Emits source for one target language from a lowered [`Schema`].
pub trait Backend {
    fn generate(&self, schema: &Schema) -> Result<Vec<GeneratedFile>, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Look up a backend by name; see [`BACKENDS`] for the accepted names.
pub fn backend(name: &str) -> Option<Box<dyn Backend>> {
    match name {
        "rust" => Some(Box::new(Rust)),
        _ => None,
    }
}
