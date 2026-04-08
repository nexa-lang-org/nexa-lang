use super::{desktop, rust, web};
use crate::application::project::{AppType, ModuleConfig, NexaProject};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("web target error: {0}")]
    Web(String),
    #[error("rust target error: {0}")]
    Rust(String),
    #[error("desktop target error: {0}")]
    Desktop(String),
    #[error("unsupported target: {0}")]
    Unsupported(String),
}

/// Build all effective platforms for a single module.
///
/// Returns one `Result` per effective platform. Web and Rust targets ignore
/// the per-platform distinction at this layer; desktop dispatches per platform.
///
/// # Example
///
/// ```no_run
/// use nexa::application::targets::build_module;
/// ```
pub fn build_module(
    project: &NexaProject,
    module: &ModuleConfig,
    entry: &Path,
) -> Vec<Result<(), BuildError>> {
    let platforms = module.effective_platforms();
    platforms
        .iter()
        .map(|platform| match module.app_type {
            AppType::Web | AppType::Package => web::build(project, module, entry)
                .map_err(|e| BuildError::Web(e.to_string())),
            AppType::Backend | AppType::Cli => rust::build(project, module, entry)
                .map_err(|e| BuildError::Rust(e.to_string())),
            AppType::Desktop => desktop::build(project, module, entry, platform)
                .map_err(|e| BuildError::Desktop(e.to_string())),
        })
        .collect()
}
