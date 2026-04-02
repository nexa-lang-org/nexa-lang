use crate::application::ports::source::SourceProvider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Test adapter: serves source files from an in-memory map.
pub struct MemSourceProvider {
    files: HashMap<PathBuf, String>,
}

impl MemSourceProvider {
    pub fn new() -> Self {
        MemSourceProvider {
            files: HashMap::new(),
        }
    }

    pub fn add(&mut self, path: impl Into<PathBuf>, content: impl Into<String>) {
        self.files.insert(path.into(), content.into());
    }
}

impl Default for MemSourceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceProvider for MemSourceProvider {
    fn read_source(&self, path: &Path) -> Result<String, std::io::Error> {
        self.files.get(path).cloned().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, path.display().to_string())
        })
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, std::io::Error> {
        if self.files.contains_key(path) {
            Ok(path.to_path_buf())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                path.display().to_string(),
            ))
        }
    }
}
